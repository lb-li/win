#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};

use fsearch_core::engine::IndexEngine;
use fsearch_core::index::snapshot::{IndexSnapshot, Store};
use fsearch_core::index::NamePool;

mod actions;
mod bridge;
mod debounce;
mod tokens;

use debounce::Debouncer;

slint::include_modules!();

fn placeholder_snapshot() -> IndexSnapshot {
    IndexSnapshot::new(
        0,
        0,
        0,
        0,
        1,
        Store::Owned {
            records: Vec::new(),
            names: NamePool::new(),
        },
    )
}

fn to_ui_row(row: bridge::Row) -> ResultRow {
    ResultRow {
        before: row.before.into(),
        matched: row.matched.into(),
        after: row.after.into(),
        path: row.path.into(),
        size: row.size.into(),
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let engine = Arc::new(IndexEngine::new(placeholder_snapshot()));
    let ui = AppWindow::new()?;
    let ui_weak = ui.as_weak();

    let debouncer = Rc::new(RefCell::new(Debouncer::new(Duration::from_millis(30))));
    let pending: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    {
        let debouncer = debouncer.clone();
        let pending = pending.clone();
        ui.on_search(move |text| {
            *pending.borrow_mut() = Some(text.to_string());
            debouncer.borrow_mut().on_input(Instant::now());
        });
    }

    let timer = Timer::default();
    {
        let debouncer = debouncer.clone();
        let pending = pending.clone();
        let engine = engine.clone();
        let weak = ui_weak.clone();
        timer.start(TimerMode::Repeated, Duration::from_millis(30), move || {
            if !debouncer.borrow_mut().should_fire(Instant::now()) {
                return;
            }
            let text = pending.borrow_mut().take().unwrap_or_default();
            let engine = engine.clone();
            let weak = weak.clone();
            std::thread::spawn(move || {
                let snapshot = engine.current();
                let (_generation, hits) = engine.query(&text, 200);
                let rows: Vec<ResultRow> = bridge::rows_from_hits(&snapshot, 'C', &hits)
                    .into_iter()
                    .map(to_ui_row)
                    .collect();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_rows(ModelRc::new(VecModel::from(rows)));
                        ui.set_selected(0);
                    }
                });
            });
        });
    }

    {
        let weak = ui_weak.clone();
        ui.on_move_selection(move |delta| {
            if let Some(ui) = weak.upgrade() {
                let count = ui.get_rows().row_count() as i32;
                let next = (ui.get_selected() + delta).clamp(0, count.saturating_sub(1));
                ui.set_selected(next);
            }
        });
    }

    {
        let weak = ui_weak.clone();
        ui.on_activate(move |_index| {
            if let Some(ui) = weak.upgrade() {
                let index = ui.get_selected().max(0) as usize;
                if let Some(row) = ui.get_rows().row_data(index) {
                    let _ = actions::open_path(&row.path);
                }
            }
        });
    }

    {
        let weak = ui_weak.clone();
        ui.on_reveal_selected(move || {
            if let Some(ui) = weak.upgrade() {
                let index = ui.get_selected().max(0) as usize;
                if let Some(row) = ui.get_rows().row_data(index) {
                    let _ = actions::reveal_path(&row.path);
                }
            }
        });
    }

    {
        let weak = ui_weak.clone();
        ui.on_copy_selected(move || {
            if let Some(ui) = weak.upgrade() {
                let index = ui.get_selected().max(0) as usize;
                if let Some(row) = ui.get_rows().row_data(index) {
                    let _ = actions::copy_path(&row.path);
                }
            }
        });
    }

    ui.on_close_requested(|| std::process::exit(0));

    #[cfg(windows)]
    {
        spawn_indexer(engine.clone());
    }

    let result = ui.run();
    drop(timer);
    result
}

#[cfg(windows)]
fn spawn_indexer(engine: Arc<IndexEngine>) {
    use fsearch_core::collector::Collector;
    use fsearch_core::index::build::IndexBuilder;

    std::thread::spawn(move || {
        let volumes = fsearch_win::volume::list_volumes();
        let Some(volume) = volumes.first() else {
            return;
        };
        let scanner = fsearch_win::mft::MftScanner::new(volume.letter);
        let Ok(batch) = scanner.scan() else {
            return;
        };
        let mut builder = IndexBuilder::with_capacity(0, batch.next_usn, batch.records.len());
        for record in batch.records {
            builder.extend(record);
        }
        let snapshot = builder.finish();
        let next_generation = engine.generation() + 1;
        engine.replace(snapshot, next_generation);
    });
}
