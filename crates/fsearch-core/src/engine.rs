use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use rayon::ThreadPool;

use crate::index::snapshot::IndexSnapshot;
use crate::query::rank::{query_parallel, Hit};

pub struct IndexEngine {
    index: ArcSwap<IndexSnapshot>,
    generation: AtomicU64,
    index_pool: ThreadPool,
    query_pool: ThreadPool,
}

impl IndexEngine {
    pub fn new(initial: IndexSnapshot) -> Self {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let index_threads = (cores / 2).max(1);
        let query_threads = (cores.saturating_sub(1)).clamp(1, 8);
        let generation = initial.generation;

        Self {
            index: ArcSwap::from_pointee(initial),
            generation: AtomicU64::new(generation),
            index_pool: rayon::ThreadPoolBuilder::new()
                .num_threads(index_threads)
                .thread_name(|i| format!("fsearch-index-{i}"))
                .build()
                .expect("index pool"),
            query_pool: rayon::ThreadPoolBuilder::new()
                .num_threads(query_threads)
                .thread_name(|i| format!("fsearch-query-{i}"))
                .build()
                .expect("query pool"),
        }
    }

    pub fn current(&self) -> Arc<IndexSnapshot> {
        self.index.load_full()
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn replace(&self, snapshot: IndexSnapshot, generation: u64) {
        self.index.store(Arc::new(snapshot));
        self.generation.store(generation, Ordering::Release);
    }

    pub fn query(&self, text: &str, limit: usize) -> (u64, Vec<Hit>) {
        let snapshot = self.index.load_full();
        let generation = self.generation();
        let hits = query_parallel(&self.query_pool, &snapshot, text, limit);
        (generation, hits)
    }

    pub fn index_pool(&self) -> &ThreadPool {
        &self.index_pool
    }

    pub fn query_pool(&self) -> &ThreadPool {
        &self.query_pool
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::RawRecord;
    use crate::index::build::IndexBuilder;

    fn snap(tag: &str) -> IndexSnapshot {
        let mut b = IndexBuilder::new(0, 0);
        b.extend(RawRecord {
            file_id: 5,
            parent_file_id: 5,
            name: "\\".into(),
            attributes: 0x10,
        });
        b.extend(RawRecord {
            file_id: 9,
            parent_file_id: 5,
            name: format!("{tag}.txt"),
            attributes: 0,
        });
        b.finish()
    }

    #[test]
    fn replace_swaps_snapshot_and_bumps_generation() {
        let engine = IndexEngine::new(snap("alpha"));
        let g1 = engine.generation();
        assert_eq!(engine.query("alpha", 10).1.len(), 1);
        engine.replace(snap("beta"), g1 + 1);
        assert_eq!(engine.query("alpha", 10).1.len(), 0);
        assert_eq!(engine.query("beta", 10).1.len(), 1);
        assert!(engine.generation() > g1);
    }

    #[test]
    fn query_returns_generation_of_snapshot_used() {
        let engine = IndexEngine::new(snap("alpha"));
        let (generation, hits) = engine.query("alpha", 10);
        assert_eq!(generation, engine.generation());
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn parallel_matches_serial() {
        let engine = IndexEngine::new(snap("alpha"));
        let serial = crate::query::rank::query(&engine.current(), "alpha", 10);
        let parallel = engine.query("alpha", 10).1;
        let ser: Vec<u32> = serial.iter().map(|h| h.record_idx).collect();
        let par: Vec<u32> = parallel.iter().map(|h| h.record_idx).collect();
        assert_eq!(ser, par);
    }
}
