# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.10.1] - 2026-10-09

Cheaper allocation tracking.

### Changed

- `alloc-tracking` now requires `mod-alloc` 1.0.1. The allocator that
  `install_global_allocator!()` installs used to walk stack frames and
  record a call site on every allocation, data dev-bench never read.
  `mod-alloc` 1.0.1 records call sites only while a
  `dhat_compat::Profiler` is alive, and dev-bench never creates one, so
  tracking now only updates counters. Measured in `mod-alloc`: a 64-byte
  alloc + free went from about 59.5 ns to 50.5 ns. Allocation counts and
  byte totals are unchanged.

### Documentation

- The `alloc` module docs describe the tracking cost and what a
  `Profiler` adds.

[0.10.1]: https://github.com/jamesgober/dev-bench/releases/tag/v0.10.1

## [0.10.0] - 2026-10-09

`mod-alloc` 1.0, MSRV 1.75, and statistics and baseline-store fixes
from a review pass. This replaces the unreleased 0.9.8: the `mod-alloc`
bump is a breaking change for one documented setup (see below), so it
ships as 0.10.0 instead of a patch.

### Breaking

- **`mod-alloc` dependency bumped from `^0.9` to `^1`.** 0.9.7's README
  told `alloc-tracking` users to add `mod-alloc` themselves and use
  `mod_alloc::dhat_compat::Profiler` directly. With dev-bench on
  `mod-alloc` 1.x, a project that still depends on `mod-alloc` 0.9 ends
  up with two copies: the allocator installed by
  `install_global_allocator!()` comes from 1.x while its `Profiler`
  comes from 0.9, and the profile is silently empty. Bump your own
  `mod-alloc` dependency to `1`, or drop it and use dev-bench's API.
  The macro itself needs no direct `mod-alloc` dependency.
- **p50 / p99 values change for some sample counts** (see Fixed). The
  statistic definitions were documented as pinned; the code now matches
  them, so the numbers move.

### Added

- `AllocationStats::since(&earlier)` to measure the allocations of one
  region. `mod-alloc`'s counters run from process start and a
  `Profiler` does not reset them.
- `VERSION` constant with the crate version as compiled, so tools that
  bundle this crate can report what is actually linked.

### Fixed

- p50, p99, `percentile` and `mad` now use nearest-rank as documented.
  The code used `floor(n * q)`, one rank high for even sample counts
  (p50) and whenever `n * 0.99` was a whole number (p99). NaN `q` gives
  the smallest sample, and float error such as `0.07 * 100` no longer
  bumps the rank.
- The mean truncated the sample count above `u32::MAX` and divided by
  zero at exactly 2^32 samples; it now divides in u128 nanoseconds.
- `run_for(Duration::MAX)` panicked on `Instant` overflow.
- `iter` now passes the closure's result through `black_box` inside the
  timed region, so the optimizer cannot drop the measured work. No
  measurable overhead.
- Percent thresholds: a NaN or infinite percent passed every run, and a
  zero baseline mean gave an infinite-percent Fail (or could never trip
  `ThroughputDropPct`). Both now give Skip. Zero against zero still
  passes.
- Baseline store:
  - A scope of `..` escaped the store root.
  - Names that sanitize to the same file name (`a/b` and `a_b`, or two
    non-ASCII names) overwrote each other, and Windows device names
    (`con`, `nul.txt`) or names over 255 bytes failed to save. Such
    names now get a stable hash suffix. Files written by earlier
    versions are still read, as long as the name stored inside matches.
  - Saves used one fixed temp file name, so concurrent saves collided.
    Each save now writes a unique temp file, syncs it, renames it into
    place (retrying on Windows when another save replaces the same file
    at that moment) and removes it on error.
  - A NaN or infinite `ops_per_sec` was written as `null`, leaving a
    file that could never be loaded. `save` now rejects it.
- A non-finite allocation `pct_threshold` gives Skip instead of passing.

### Changed

- **MSRV lowered from `1.85` to `1.75`.** `mod-alloc` 1.0 is MSRV 1.75
  and dev-report 0.9.7 dropped to 1.75, so nothing holds dev-bench at
  1.85. CI's MSRV job now builds on 1.75 against an MSRV-compatible
  lockfile.
- The local `mod-alloc` path dependency now points at
  `../../mod-collection/mod-alloc`, where that crate actually lives,
  and CI clones it there. Published builds use crates.io as before.

### Documentation

- `iter_with_count` said the reported mean was per iteration; it is per
  batch.
- `cv` is documented as using the population standard deviation.
- The allocation counters are documented as process-wide.
- The `install_global_allocator!()` and README allocation examples no
  longer require a direct `mod-alloc` dependency.
- README: version snippets, Status section and MSRV section updated.

[0.10.0]: https://github.com/jamesgober/dev-bench/releases/tag/v0.10.0

## [0.9.7] - 2026-05-18

### Changed

- **`alloc-tracking` backend swapped from `dhat = "0.3"` to
  `mod-alloc`'s `dhat_compat` surface.** The public API
  (`AllocationStats`, `AllocationStats::snapshot()`, the
  `install_global_allocator!()` macro) is unchanged; downstream
  callers that use this crate's macro need no edits. Callers that
  reach for `dhat::Profiler` or `dhat::HeapStats` directly should
  switch to `use mod_alloc::dhat_compat as dhat;` — the surface is
  method-for-method compatible (see `MIGRATING_FROM_DHAT.md` in
  the mod-alloc repo for the full mapping).
- **`install_global_allocator!` now installs
  `mod_alloc::dhat_compat::Alloc`.** Same `#[global_allocator] static`
  shape; same DHAT-viewer-compatible JSON output on `Profiler`
  drop.
- **`AllocationStats::snapshot()` no longer panics outside a
  Profiler scope.** `dhat-rs`'s historical panic on `HeapStats::get()`
  without a live Profiler is gone; the snapshot returns zeros if
  no global allocator is installed.

### Removed

- **`dhat` dependency dropped.** The transitive
  `dhat -> backtrace -> addr2line` chain (which forced Rust 1.85+
  through `addr2line 0.25.1`) is no longer pulled in by
  `alloc-tracking`.

### Added

- `mod-alloc = { version = "0.9", features = ["dhat-compat"], optional = true }`
  as the new `alloc-tracking` backend. MSRV 1.75, pure-Rust, no
  FFI, zero runtime deps on the alloc hot path.

### MSRV

- Held at `1.85`. The dhat-side blocker is gone, but the
  `dev-report` sibling still requires 1.85; dropping dev-bench's
  MSRV to 1.75 waits on a future `dev-report` milestone.

### Migration

No action required for callers that use this crate's
`install_global_allocator!()` macro. Callers that referenced the
`dhat` crate directly in their own code (separate from this
crate's macro expansion) should swap their imports:

```rust
// Before
use dhat;

// After
use mod_alloc::dhat_compat as dhat;
```

Cargo.toml swap is automatic when you bump `dev-bench` to 0.9.7 —
the `alloc-tracking` feature now resolves to `mod-alloc` instead
of `dhat`.

### Notes

- JSON output from the new backend's `Profiler` loads in the same
  upstream `dh_view.html` viewer (shipped with Valgrind).
- `dev-bench` does not consume per-call-site backtrace reports;
  it only reads `HeapStats` counters. The mod-alloc walker's
  shallow-trace gap on stock-std release builds (Windows) does
  not affect `alloc-tracking` output.

[0.9.7]: https://github.com/jamesgober/dev-bench/releases/tag/v0.9.7

## [0.9.6] - 2026-05-12

Skip-release to clean up a premature `v0.9.5` GH tag that never reached crates.io. The compiled artifact is the v0.9.4 source plus the `Cargo.lock`-untrack repository-hygiene commit; the version label moves to `0.9.6` so the published version sequence on crates.io stays monotonic after the orphan tag is deleted. No code or behavior change from v0.9.4.

### Changed

- Stopped tracking `Cargo.lock`. Library crates conventionally do not commit lock files — the lock is a snapshot of one resolution and downstream consumers ignore it anyway. Tracking it was generating spurious dirty-tree noise every time a path-dep sibling bumped version. `Cargo.lock` is now in `.gitignore`; `git rm --cached Cargo.lock` retired the tracked copy.

### Notes

- The compiled crate is byte-equivalent to v0.9.4 plus the lock-file untracking.
- v0.9.5 never existed on crates.io. A premature `v0.9.5` GitHub tag/release was created without bumping `Cargo.toml` first; that tag has been deleted as part of this release.
- No new dependencies, no MSRV change.

[0.9.6]: https://github.com/jamesgober/dev-bench/releases/tag/v0.9.6

## [0.9.4] - 2026-05-12

Documentation and SEO pass. No code changes.

### Changed

- README header standardized to match the collection-wide template: Rust logo image, MSRV badge between CI and docs.rs, copyright block at bottom.
- Subtitle now reads `BENCHMARK & REGRESSION DETECTION FOR RUST` (was `PERFORMANCE MEASUREMENT FOR RUST`). More specific and search-tighter.
- Tagline rewritten to lead with the developer outcome (measure, baseline, catch regressions before they ship) instead of the part-of-suite framing.
- `## What it does` no longer leads with the AI-agent framing; the positioning vs. `criterion`/`divan` is more direct.
- `## The dev-* suite` retitled to `The dev-* collection` with the full 14-crate map.
- `Cargo.toml` description rewritten: lists actual feature surface (percentile stats, baselines, threshold gating, CI-gateable verdicts).
- `Cargo.toml` keywords retuned: dropped `verification` and `ai-tools`, added `profiling` and `ci` for crates.io search.

### Added

- "Part of the `dev-*` verification collection" block on the README, under the intro, linking the umbrella `dev-tools` crate.

[0.9.4]: https://github.com/jamesgober/dev-bench/releases/tag/v0.9.4

## [0.9.3] - 2026-05-12

### Added

- `examples/basic_benchmark.rs` — minimal runnable demonstration of the `Benchmark::new` → `iter` → `finish` flow, printing the headline statistics (`mean`, `p50`, `p99`, `ops_per_sec`, `cv`).

### Changed

- CI: `actions/checkout` bumped from `v4` to `v5` (removes Node 20 deprecation warnings).

[0.9.3]: https://github.com/jamesgober/dev-bench/releases/tag/v0.9.3

## [0.9.2] - 2026-05-10

### Added

- `Benchmark::run_for(budget, closure)` — wall-clock-bounded benchmark mode. Runs the closure repeatedly until `budget` elapses, recording one sample per iteration. Pairs naturally with iteration-based `iter` for benchmarks where you want "for N seconds" semantics.
- `BenchmarkResult::histogram(bucket_count)` returning a `Vec<HistogramBin>` with uniform-width bins covering `[min, max]`. Surfaces bimodality, outlier tails, and warmup effects that mean/percentile alone hide.
- New `HistogramBin { lower, upper, count }` type.

[0.9.2]: https://github.com/jamesgober/dev-bench/releases/tag/v0.9.2

## [0.9.1] - 2026-05-09

### Added

- `BenchmarkResult` extra statistics: `min()`, `max()`, `stddev()` (sample, Bessel's correction), `mad()` (median absolute deviation), `p90()`, `p999()`, and a generic `percentile(q)` accessor.
- `install_global_allocator!()` macro (gated by `alloc-tracking`) that expands to the `#[global_allocator] static ALLOC: dhat::Alloc = dhat::Alloc;` declaration users would otherwise have to write themselves. Also re-exports `dhat` privately as `__dhat` for the macro to reference.

### Fixed

- Broken intra-doc link `[`alloc`]` in the crate-level docstring would warn under `cargo doc` when the `alloc-tracking` feature is disabled. The link is now a plain code span.

[0.9.1]: https://github.com/jamesgober/dev-bench/releases/tag/v0.9.1

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

[Unreleased]: https://github.com/jamesgober/dev-bench/compare/v0.9.3...HEAD
[0.1.0]: https://github.com/jamesgober/dev-bench/releases/tag/v0.1.0
