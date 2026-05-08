<h1 align="center">
    <strong>dev-bench</strong>
    <br>
    <sup><sub>PERFORMANCE MEASUREMENT FOR RUST</sub></sup>
</h1>

<p align="center">
    <a href="https://crates.io/crates/dev-bench"><img alt="crates.io" src="https://img.shields.io/crates/v/dev-bench.svg"></a>
    <a href="https://crates.io/crates/dev-bench"><img alt="downloads" src="https://img.shields.io/crates/d/dev-bench.svg"></a>
    <a href="https://github.com/jamesgober/dev-bench/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/jamesgober/dev-bench/actions/workflows/ci.yml/badge.svg"></a>
    <a href="https://docs.rs/dev-bench"><img alt="docs.rs" src="https://docs.rs/dev-bench/badge.svg"></a>
</p>

<p align="center">
    Performance measurement and regression detection.<br>
    Part of the <code>dev-*</code> verification suite.
</p>

---

## What it does

Measures performance and produces verdicts the same way the rest of the
`dev-*` suite does: machine-readable, stored as `dev-report` results.

Designed for AI agents and CI gating, not for interactive profiling.
For interactive profiling, use `criterion` or `divan`.

## Quick start

```toml
[dependencies]
dev-bench = "0.9"
```

```rust
use dev_bench::{Benchmark, Threshold};

let mut b = Benchmark::new("parse_query");
for _ in 0..1000 {
    b.iter(|| std::hint::black_box(40 + 2));
}

let result = b.finish();
let threshold = Threshold::regression_pct(10.0);

// If you have a stored baseline mean from a previous run, pass it in.
// Returns a CheckResult that can be pushed into a dev_report::Report.
let _check = result.compare_against_baseline(None, threshold);
```

The returned `CheckResult` carries the `bench` tag and numeric
`Evidence` for `mean_ns`, `baseline_ns`, `p50_ns`, `p99_ns`, `cv`,
`ops_per_sec`, `samples`, and `iterations_recorded` — no parsing of
free-form detail strings.

## Variance-aware verdicts

Noisy machines produce false regressions. `dev-bench` computes the
coefficient of variation alongside the mean and downgrades regressions
within the noise band from `Fail` to `Warn`:

```rust
use dev_bench::{Benchmark, CompareOptions, Threshold};
use std::time::Duration;

let mut b = Benchmark::new("hot");
for _ in 0..30 { b.iter(|| std::hint::black_box(1 + 1)); }
let r = b.finish();

let opts = CompareOptions {
    baseline_mean: Some(Duration::from_nanos(1000)),
    threshold: Threshold::regression_pct(10.0),
    min_samples: 30,
    allow_cv_noise_band: true,
};
let _check = r.compare_with_options(&opts);
```

## Throughput

For sub-microsecond operations where `Instant::now()` overhead would
dominate per-iter timing, batch with `iter_with_count`:

```rust
use dev_bench::{Benchmark, Threshold};

let mut b = Benchmark::new("hot");
b.iter_with_count(100_000, || {
    std::hint::black_box(1 + 1);
});
let r = b.finish();
println!("{} ops/sec", r.ops_per_sec());
```

## Baseline storage

Baselines persist via the `BaselineStore` trait. A JSON file backend
ships out of the box:

```rust
use dev_bench::{Baseline, BaselineStore, JsonFileBaselineStore};

let store = JsonFileBaselineStore::new("./baselines");
let baseline = Baseline {
    name: "parse_query".into(),
    mean_ns: 1234,
    samples: 1000,
    ops_per_sec: 810_000.0,
};
store.save("main", &baseline).unwrap();

if let Some(b) = store.load("main", "parse_query").unwrap() {
    // use b.mean() in compare_against_baseline(...)
}
```

Saves are atomic (write-temp-rename); loads tolerate missing files.

## Producer trait

To plug into `dev-report::MultiReport` aggregation alongside other
producers, wrap a benchmark in `BenchProducer`:

```rust
use dev_bench::{BenchProducer, Benchmark, BenchmarkResult, Threshold};
use dev_report::Producer;

fn run() -> BenchmarkResult {
    let mut b = Benchmark::new("parse");
    for _ in 0..100 { b.iter(|| std::hint::black_box(1 + 1)); }
    b.finish()
}

let producer = BenchProducer::new(run, "0.1.0", None, Threshold::regression_pct(10.0));
let report = producer.produce();   // dev_report::Report
```

## Allocation tracking (opt-in)

```toml
[dependencies]
dev-bench = { version = "0.9", features = ["alloc-tracking"] }
```

```rust,ignore
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

let _profiler = dhat::Profiler::new_heap();
// ... run benchmarked code ...
let stats = dev_bench::alloc::AllocationStats::snapshot();
let check = stats.compare_against_baseline("parse", baseline_alloc, 10.0);
```

The dhat allocator changes timing characteristics, so do **not**
combine timing thresholds with allocation thresholds in the same
invocation.

## Design choices

- **Output is a `CheckResult`**, not a stdout dump. Agents can read it.
- **Regression-first**, not absolute-perf-first. The interesting question
  is "did this change make it slower," not "is this the fastest in the
  world."
- **Configurable thresholds**: percent-based, absolute-nanosecond, or
  throughput-drop-percent.
- **Variance-aware**: regressions within the CV noise band are flagged
  as `Warn`, not `Fail`.

## The `dev-*` suite

`dev-bench` is one of the producer crates. See
[`dev-tools`](https://github.com/jamesgober/dev-tools) for the umbrella
crate and [`dev-report`](https://github.com/jamesgober/dev-report) for
the schema all results conform to.

## Status

`v0.9.x` is the pre-1.0 stabilization line. APIs are expected to be
near-final; minor adjustments may still happen ahead of `1.0`. The
statistic definitions (`mean`, `p50`, `p99`) are pinned and will not
change.

## Minimum supported Rust version

`1.85` — pinned in `Cargo.toml` via `rust-version` and verified by
the MSRV job in CI. (Bumped from 1.75 because the `alloc-tracking`
feature pulls `dhat` → `addr2line`, which requires Rust 1.81+, and
sibling crates require `edition2024` (1.85+).)

## License

Apache-2.0. See [LICENSE](LICENSE).
