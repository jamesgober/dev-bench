# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/jamesgober/dev-bench/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/jamesgober/dev-bench/releases/tag/v0.1.0
