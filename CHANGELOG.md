# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.9.0] - 2026-05-08

### Added

#### Adoption of dev-report 0.9

- Bumped `dev-report` dep to `0.9`.
- Every non-`Skip` `CheckResult` from `compare_*` now carries the `bench` tag and structured numeric `Evidence`: `mean_ns`, `baseline_ns`, `p50_ns`, `p99_ns`, `cv`, `ops_per_sec`, `samples`, `iterations_recorded`.
- Regression-flagged checks additionally carry the `regression` tag.

#### Throughput

- `BenchmarkResult::ops_per_sec()` returning `iterations_recorded / total_elapsed_seconds`.
- `BenchmarkResult::iterations_recorded` and `total_elapsed` fields.
- `Benchmark::iter_with_count(n, f)` for batched workloads (records ONE sample for `n` ops).
- `Threshold::ThroughputDropPct(pct)` variant.
- `Threshold::throughput_drop_pct` constructor.

#### Variance awareness

- `BenchmarkResult::cv` (coefficient of variation, stddev / mean).
- `CompareOptions` with `min_samples` and `allow_cv_noise_band` controls.
- `BenchmarkResult::compare_with_options(&CompareOptions)`.
- Regressions inside `baseline_ns * cv` are downgraded from `Fail` to `Warn` when `allow_cv_noise_band` is true.
- Skips with detail when `samples.len() < min_samples`.

#### Baseline storage

- `Baseline` struct (serde-roundtrippable: `name`, `mean_ns`, `samples`, `ops_per_sec`).
- `BaselineStore` trait with `load(scope, name)` and `save(scope, baseline)`.
- `JsonFileBaselineStore`: filesystem JSON backend keyed by `(scope, name)`.
- Atomic save via write-temp-rename; tolerant load (returns `Ok(None)` on missing).
- Path sanitization on `scope` and `name` to prevent traversal.

#### Allocation tracking (opt-in)

- `alloc-tracking` feature flag (off by default).
- `dev_bench::alloc::AllocationStats` capturing `total_bytes`, `total_blocks`, `peak_bytes`, `peak_blocks` from `dhat::HeapStats`.
- `AllocationStats::compare_against_baseline` returning a `CheckResult` with `alloc` tag and numeric evidence.

#### Producer trait integration

- `BenchProducer<F>` adapter implementing `dev_report::Producer`.
- `BenchmarkResult::into_report` shortcut producing a single-check finalized `Report`.

### Documentation

- All public items have rustdoc with at least one example.
- REPS.md expanded: §4 (cv, ops_per_sec definitions), §5 (verdict semantics, required evidence), §6 (baseline storage), §7 (allocation tracking), §8 (producer integration).

[0.9.0]: https://github.com/jamesgober/dev-bench/releases/tag/v0.9.0

## [0.1.0] - 2026-05-07

### Added

- Initial crate skeleton.
- `Benchmark` runner with sample collection.
- `BenchmarkResult` with mean, p50, p99 statistics.
- `Threshold::RegressionPct` and `Threshold::RegressionAbsoluteNs`.
- `compare_against_baseline` returning a `dev-report::CheckResult`.
- Smoke tests covering empty, no-baseline, and pass paths.

### Note

This is a name-claim release. Public API will expand significantly
in `0.2.x` and `0.3.x` for throughput, allocation tracking, and
baseline storage.

[Unreleased]: https://github.com/jamesgober/dev-bench/compare/v0.9.0...HEAD
[0.1.0]: https://github.com/jamesgober/dev-bench/releases/tag/v0.1.0
