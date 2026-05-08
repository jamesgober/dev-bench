<h1 align="center">
    <strong>dev-bench</strong>
    <br>
    <sup><sub>PERFORMANCE MEASUREMENT FOR RUST</sub></sup>
</h1>

<p align="center">
    <a href="https://crates.io/crates/dev-bench"><img alt="crates.io" src="https://img.shields.io/crates/v/dev-bench.svg"></a>
    <a href="https://docs.rs/dev-bench"><img alt="docs.rs" src="https://docs.rs/dev-bench/badge.svg"></a>
    <a href="https://github.com/jamesgober/dev-bench/blob/main/LICENSE"><img alt="License" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
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
dev-bench = "0.1"
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
let check = result.compare_against_baseline(None, threshold);
```

## Design choices

- **Output is a `CheckResult`**, not a stdout dump. Agents can read it.
- **Regression-first**, not absolute-perf-first. The interesting question
  is "did this change make it slower," not "is this the fastest in the
  world."
- **Configurable thresholds**: percent-based or absolute-nanosecond.

## What's planned

- Throughput measurements (ops/sec).
- Allocation tracking via `dhat` integration (feature-gated).
- Baseline storage formats (JSON files keyed by git SHA).
- Comparison helpers that read baselines from disk.

## The `dev-*` suite

`dev-bench` is one of the producer crates. See
[`dev-tools`](https://github.com/jamesgober/dev-tools) for the umbrella
crate and [`dev-report`](https://github.com/jamesgober/dev-report) for
the schema all results conform to.

## License

Apache-2.0. See [LICENSE](LICENSE).
