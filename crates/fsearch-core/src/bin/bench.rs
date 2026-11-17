use std::time::Instant;

use fsearch_core::collector::RawRecord;
use fsearch_core::engine::IndexEngine;
use fsearch_core::index::build::IndexBuilder;

fn synthetic(count: usize) -> IndexEngine {
    let mut builder = IndexBuilder::with_capacity(0, 0, count + 1);
    builder.extend(RawRecord {
        file_id: 5,
        parent_file_id: 5,
        name: "\\".to_string(),
        attributes: 0x10,
    });
    let names = [
        "src", "tests", "docs", "main", "lib", "report", "notes", "image",
    ];
    let exts = ["rs", "txt", "md", "png", "json"];
    for i in 0..count {
        builder.extend(RawRecord {
            file_id: (i as u64) + 10,
            parent_file_id: 5,
            name: format!("{}_{}.{}", names[i % names.len()], i, exts[i % exts.len()]),
            attributes: 0,
        });
    }
    IndexEngine::new(builder.finish())
}

fn main() {
    let count: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000);

    let build_start = Instant::now();
    let engine = synthetic(count);
    let build_ms = build_start.elapsed().as_millis();

    let queries = ["main", "report_1", "*.rs", "notes json", "src_"];
    let mut samples = Vec::new();
    for _ in 0..20 {
        for q in queries {
            let start = Instant::now();
            let (_generation, hits) = engine.query(q, 200);
            samples.push(start.elapsed().as_micros() as u64);
            std::hint::black_box(hits);
        }
    }
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2];
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];

    println!("records      : {count}");
    println!("build        : {build_ms} ms");
    println!("query p50    : {:.2} ms", p50 as f64 / 1000.0);
    println!("query p99    : {:.2} ms", p99 as f64 / 1000.0);
    println!(
        "approx index : {:.1} MB",
        (count * 16 + count * 20) as f64 / (1024.0 * 1024.0)
    );
}
