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
- All statistics MUST be computed losslessly (no precomputed bins).

## 5. Regression detection

A regression is detected when the current measurement exceeds the
baseline by more than the configured threshold.

- `RegressionPct(pct)`: current_mean > baseline_mean * (1 + pct/100)
- `RegressionAbsoluteNs(ns)`: (current_mean - baseline_mean) > ns

A regression MUST emit a `CheckResult` with verdict `Fail` and
severity `Warning`. Severity escalation to `Error` is the consumer's
choice via report aggregation rules.

## 6. Baselines

In `0.1.x`, baselines are passed in as `Option<Duration>`. In `0.3.x`
or later, baselines MAY be loaded from disk via a configurable
storage backend. The backend MUST NOT be required for basic use.
