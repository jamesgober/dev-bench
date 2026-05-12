//! Run a tiny benchmark and print the resulting percentiles.
//!
//! ```text
//! cargo run --example basic_benchmark --release
//! ```
//!
//! Demonstrates the core `Benchmark::new` → `iter` → `finish` flow and
//! shows the headline statistics (`mean`, `p50`, `p99`, `ops_per_sec`)
//! produced by `BenchmarkResult`.

use dev_bench::Benchmark;

fn workload() -> u64 {
    let mut acc: u64 = 0;
    for i in 0..1_000 {
        acc = std::hint::black_box(acc.wrapping_add(i));
    }
    acc
}

fn main() {
    let mut bench = Benchmark::new("trivial_add");
    for _ in 0..10_000 {
        bench.iter(|| {
            std::hint::black_box(workload());
        });
    }
    let result = bench.finish();
    println!("name:        {}", result.name);
    println!("samples:     {}", result.samples.len());
    println!("mean:        {:?}", result.mean);
    println!("p50:         {:?}", result.p50);
    println!("p99:         {:?}", result.p99);
    println!("ops_per_sec: {:.0}", result.ops_per_sec());
    println!("cv:          {:.4}", result.cv);
}
