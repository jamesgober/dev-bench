# dev-bench — Project Specification (REPS)

> Rust Engineering Project Specification.
> Normative language follows RFC 2119.

## 1. Purpose

`dev-bench` MUST measure code performance and detect regressions
against a stored baseline. Output MUST be a `dev-report::CheckResult`
or `dev-report::Report`, never plain stdout.

## 2. Scope

This crate MUST provide:

- A `Benchmark` runner with sample collection.
- A `BenchmarkResult` with at least mean, p50, and p99 statistics.
- Threshold types for regression detection (percent and absolute).
- A comparison API that returns a `dev-report::CheckResult`.

This crate SHOULD provide (in future versions):

- Throughput measurement (ops/sec).
- Allocation tracking (feature-gated).
- Baseline storage (JSON keyed by git SHA or commit ref).
- Comparison helpers that read baselines from disk.

This crate MUST NOT:

- Run interactive profilers (use `criterion` or `divan`).
- Replace `criterion`. The two coexist for different audiences.
- Produce HTML reports. Output is JSON-via-dev-report only.

## 3. Sample collection

Each `iter` call MUST capture a single duration sample. The runner
MUST NOT re-order or batch iterations transparently. If batching is
needed (e.g. for sub-microsecond ops), it MUST be opt-in via a
distinct API.

## 4. Statistics

- `mean` MUST be the arithmetic mean of all samples.
- `p50` MUST be the median sample.
- `p99` MUST be the 99th percentile sample.
- `cv` MUST be `stddev / mean`, computed across all samples.
- `ops_per_sec` MUST be `iterations_recorded / total_elapsed_seconds`.
- All statistics MUST be computed losslessly (no precomputed bins).

The first three (`mean`, `p50`, `p99`) are the **immutable contract**;
their definitions MUST NOT change in any version. `cv` and
`ops_per_sec` are additive at v0.5.x and v0.2.x respectively.

## 5. Regression detection

A regression is detected when the current measurement exceeds the
baseline by more than the configured threshold.

- `RegressionPct(pct)`: `current_mean > baseline_mean * (1 + pct/100)`
- `RegressionAbsoluteNs(ns)`: `(current_mean - baseline_mean) > ns`
- `ThroughputDropPct(pct)`: `current_ops_per_sec < baseline_ops_per_sec * (1 - pct/100)`,
  where `baseline_ops_per_sec = 1.0 / baseline_mean_secs`.

### 5.1 Verdict semantics

- A regression outside the CV noise band MUST emit a `CheckResult`
  with verdict `Fail` and severity `Warning`.
- A regression inside the CV noise band (i.e. the duration delta is
  no greater than `baseline_mean * cv`) MUST emit verdict `Warn`
  rather than `Fail`, when `allow_cv_noise_band` is true.
- A comparison with no baseline MUST emit verdict `Skip`.
- A comparison with fewer samples than `min_samples` MUST emit
  verdict `Skip` with a `min_samples` detail.
- Severity escalation to `Error` is the consumer's choice via report
  aggregation rules.

### 5.2 Required evidence

Every non-`Skip` `CheckResult` emitted by `compare_*` MUST carry the
following numeric `Evidence` (using `dev-report::Evidence::numeric`):

- `mean_ns`
- `baseline_ns` (when a baseline was provided)
- `p50_ns`
- `p99_ns`
- `cv`
- `ops_per_sec`
- `samples`
- `iterations_recorded`

The `CheckResult` MUST carry the tag `bench`. Regression-flagged
checks MUST additionally carry the tag `regression`.

## 6. Baselines

`0.1.x` accepted baselines as inline `Option<Duration>`. `0.4.x+`
adds a `BaselineStore` trait with a JSON file backend
(`JsonFileBaselineStore`). The trait MUST treat `(scope, name)` as
the identity of a baseline. Implementations MUST:

- Treat `load` as tolerant of missing data (return `Ok(None)`).
- Treat `save` as atomic (write-temp-rename or equivalent).

The default JSON backend keys baselines as
`<root>/<scope>/<name>.json`. `scope` and `name` are sanitized to
prevent path traversal.

The backend MUST NOT be required for basic use; passing
`Option<Duration>` directly into `compare_*` remains supported.

## 7. Allocation tracking

Allocation tracking is opt-in via the `alloc-tracking` feature flag.
When enabled:

- The crate exposes `dev_bench::alloc::AllocationStats`.
- `mod-alloc` is added as a dependency (with the `dhat-compat`
  feature). v0.9.7 swapped the backend from `dhat = "0.3"` to
  `mod-alloc`'s `dhat_compat` surface. The public API is unchanged.
- The caller is responsible for installing
  `mod_alloc::dhat_compat::Alloc` as the global allocator (via
  `dev_bench::install_global_allocator!()` or directly) and
  instantiating `mod_alloc::dhat_compat::Profiler` around the
  measured scope.
- Allocation stats MUST NOT be combined with timing thresholds in a
  single comparison invocation; the tracking allocator changes
  timing characteristics enough to invalidate the comparison.

## 8. Producer integration

This crate MUST provide a way to satisfy `dev_report::Producer`. The
provided implementation (`BenchProducer`) wraps a closure that
returns a `BenchmarkResult` and emits a single-check `Report` with
`producer = "dev-bench"`.
