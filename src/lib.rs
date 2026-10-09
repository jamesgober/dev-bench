//! # dev-bench
//!
//! Performance measurement and regression detection for Rust. Part of
//! the `dev-*` verification suite.
//!
//! `dev-bench` answers the question: did this change make the code
//! faster, slower, or stay the same? It compares current measurements
//! against a stored baseline and emits verdicts via `dev-report`.
//!
//! ## Quick example
//!
//! ```no_run
//! use dev_bench::{Benchmark, Threshold};
//!
//! let mut b = Benchmark::new("parse_query");
//! for _ in 0..1000 {
//!     b.iter(|| {
//!         std::hint::black_box(40 + 2);
//!     });
//! }
//!
//! let result = b.finish();
//! let threshold = Threshold::regression_pct(10.0);   // fail on +10%
//! let _check = result.compare_against_baseline(None, threshold);
//! ```
//!
//! ## What's measured
//!
//! Per-sample wall-clock duration captured via `Instant::now()`. From
//! the samples, `dev-bench` reports `mean`, `p50`, `p99`, `cv`, and a
//! derived `ops_per_sec` throughput. See [`BenchmarkResult`].
//!
//! ## Features
//!
//! - `alloc-tracking` (opt-in): measures allocation count and bytes
//!   alongside time, using `mod-alloc`'s `dhat_compat` surface
//!   (drop-in for `dhat-rs`). See the `alloc` module (visible
//!   in rustdoc when the feature is enabled).

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
#![warn(rust_2018_idioms)]

/// Version of this crate as compiled, taken from its `Cargo.toml`.
///
/// Lets tools that bundle this crate, such as the `dev` CLI in
/// `dev-tools`, report the version that is actually linked.
///
/// # Example
///
/// ```
/// assert!(!dev_bench::VERSION.is_empty());
/// ```
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

use std::time::{Duration, Instant};

use dev_report::{CheckResult, Evidence, Producer, Report, Severity};

#[cfg(feature = "alloc-tracking")]
#[cfg_attr(docsrs, doc(cfg(feature = "alloc-tracking")))]
pub mod alloc;

/// Re-export of `mod-alloc`'s `dhat_compat` surface for use by
/// [`install_global_allocator!`].
///
/// Kept under the historical `__dhat` name so the macro expansion
/// stays compatible across the v0.9.6 → v0.9.7 backend swap. The
/// macro reaches the allocator through `$crate::__dhat`, so callers do
/// not need their own `mod-alloc` dependency.
///
/// Hidden from rustdoc; consumers should use the macro, not this path.
/// Because it is still a `pub` path, the `mod-alloc` major version is
/// part of `dev-bench`'s public surface when `alloc-tracking` is on.
#[cfg(feature = "alloc-tracking")]
#[doc(hidden)]
pub use ::mod_alloc::dhat_compat as __dhat;

/// Install the allocation-tracking global allocator.
///
/// Available with the `alloc-tracking` feature. Invoke once, at module
/// scope, in your binary or test target. The macro expands to a
/// `#[global_allocator] static` declaration that consumers cannot
/// otherwise express without depending on `mod-alloc` directly; the
/// expansion names the type through `$crate`, so only `dev-bench` needs
/// to be in your `Cargo.toml`.
///
/// The backend is `mod-alloc`'s `dhat_compat::Alloc` (drop-in for
/// `dhat-rs`'s `dhat::Alloc`).
///
/// # Example
///
/// ```ignore
/// // in main.rs or a test target's top level:
/// dev_bench::install_global_allocator!();
///
/// use dev_bench::alloc::AllocationStats;
///
/// fn main() {
///     let before = AllocationStats::snapshot();
///     // ... benchmarked code ...
///     let used = AllocationStats::snapshot().since(&before);
///     println!("{} bytes in {} allocations", used.total_bytes, used.total_blocks);
/// }
/// ```
#[cfg(feature = "alloc-tracking")]
#[cfg_attr(docsrs, doc(cfg(feature = "alloc-tracking")))]
#[macro_export]
macro_rules! install_global_allocator {
    () => {
        #[global_allocator]
        static __DEV_BENCH_DHAT_ALLOC: $crate::__dhat::Alloc = $crate::__dhat::Alloc;
    };
}

pub mod baseline;

pub use baseline::{Baseline, BaselineStore, JsonFileBaselineStore};

/// A single benchmark run.
///
/// Collects per-iteration duration samples. Call [`Benchmark::finish`]
/// to produce a [`BenchmarkResult`].
///
/// # Example
///
/// ```
/// use dev_bench::Benchmark;
///
/// let mut b = Benchmark::new("noop");
/// for _ in 0..10 {
///     b.iter(|| std::hint::black_box(42));
/// }
/// let r = b.finish();
/// assert_eq!(r.samples.len(), 10);
/// ```
pub struct Benchmark {
    name: String,
    samples: Vec<Duration>,
    iterations_recorded: u64,
}

impl Benchmark {
    /// Begin a new benchmark with a stable name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            samples: Vec::new(),
            iterations_recorded: 0,
        }
    }

    /// Run one iteration of the benchmark, capturing the duration.
    ///
    /// Each call records exactly one sample.
    ///
    /// # Example
    ///
    /// ```
    /// use dev_bench::Benchmark;
    ///
    /// let mut b = Benchmark::new("noop");
    /// b.iter(|| std::hint::black_box(1 + 1));
    /// let r = b.finish();
    /// assert_eq!(r.samples.len(), 1);
    /// ```
    pub fn iter<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        let start = Instant::now();
        // The result goes through `black_box` inside the timed region so
        // the optimizer cannot drop the work when the caller discards
        // the return value.
        let r = std::hint::black_box(f());
        let elapsed = start.elapsed();
        self.samples.push(elapsed);
        self.iterations_recorded += 1;
        r
    }

    /// Run a closure `n` times and record ONE sample for the entire batch.
    ///
    /// Use for sub-microsecond operations where per-iteration timing
    /// would be dominated by `Instant::now()` overhead.
    ///
    /// The recorded sample is the duration of the whole batch, so
    /// `mean`, `p50`, `p99` and the other sample statistics describe
    /// batches, not single iterations. [`BenchmarkResult::ops_per_sec`]
    /// divides by [`BenchmarkResult::iterations_recorded`] and therefore
    /// reports per-iteration throughput. The closure's return value is
    /// not observed; wrap results in [`std::hint::black_box`] yourself.
    ///
    /// # Example
    ///
    /// ```
    /// use dev_bench::Benchmark;
    ///
    /// let mut b = Benchmark::new("hot");
    /// b.iter_with_count(1000, || {
    ///     std::hint::black_box(40 + 2);
    /// });
    /// let r = b.finish();
    /// assert_eq!(r.samples.len(), 1);
    /// assert_eq!(r.iterations_recorded, 1000);
    /// ```
    pub fn iter_with_count<F>(&mut self, n: u64, mut f: F)
    where
        F: FnMut(),
    {
        let start = Instant::now();
        for _ in 0..n {
            f();
        }
        let elapsed = start.elapsed();
        self.samples.push(elapsed);
        self.iterations_recorded += n;
    }

    /// Run a closure repeatedly for at most `budget` wall-clock time,
    /// recording one sample per iteration.
    ///
    /// Stops as soon as the elapsed time crosses `budget`. The
    /// closure may run slightly past the budget (the in-flight
    /// iteration completes); the recorded sample count reflects what
    /// was actually executed.
    ///
    /// Useful when you want a benchmark to run "for N seconds" rather
    /// than "for N iterations" — the per-iter cost is unknown and you
    /// just want a bounded run.
    ///
    /// # Example
    ///
    /// ```
    /// use dev_bench::Benchmark;
    /// use std::time::Duration;
    ///
    /// let mut b = Benchmark::new("hot");
    /// b.run_for(Duration::from_millis(20), || {
    ///     std::hint::black_box(1 + 1);
    /// });
    /// let r = b.finish();
    /// // At least one sample was collected.
    /// assert!(!r.samples.is_empty());
    /// ```
    pub fn run_for<F>(&mut self, budget: Duration, mut f: F)
    where
        F: FnMut(),
    {
        // Compare elapsed time instead of computing `now + budget`, which
        // panics on overflow for very large budgets such as `Duration::MAX`.
        let run_start = Instant::now();
        while run_start.elapsed() < budget {
            let start = Instant::now();
            f();
            let elapsed = start.elapsed();
            self.samples.push(elapsed);
            self.iterations_recorded += 1;
        }
    }

    /// Finalize the benchmark and produce a [`BenchmarkResult`].
    pub fn finish(self) -> BenchmarkResult {
        let n = self.samples.len();
        let total_elapsed: Duration = self.samples.iter().copied().sum();
        let mean = mean_of(total_elapsed, n);
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let p50 = nearest_rank(&sorted, 0.50);
        let p99 = nearest_rank(&sorted, 0.99);
        let cv = compute_cv(&self.samples, mean);
        BenchmarkResult {
            name: self.name,
            samples: self.samples,
            iterations_recorded: self.iterations_recorded,
            total_elapsed,
            mean,
            p50,
            p99,
            cv,
        }
    }
}

/// Mean of `n` samples whose sum is `total`. `Duration::ZERO` for `n == 0`.
///
/// Divides in `u128` nanoseconds so sample counts above `u32::MAX` are
/// handled (`Duration / u32` would truncate the count, and divide by zero
/// at exactly `2^32` samples).
fn mean_of(total: Duration, n: usize) -> Duration {
    if n == 0 {
        return Duration::ZERO;
    }
    let nanos = total.as_nanos() / n as u128;
    // The mean never exceeds the largest sample, which is a valid
    // `Duration`, so this conversion does not lose range in practice.
    let secs = (nanos / 1_000_000_000) as u64;
    let sub = (nanos % 1_000_000_000) as u32;
    Duration::new(secs, sub)
}

/// Zero-based index of the nearest-rank `q` percentile in a sorted slice of
/// length `n > 0`: rank `ceil(q * n)`, clamped to `1..=n`.
fn nearest_rank_index(n: usize, q: f64) -> usize {
    debug_assert!(n > 0);
    // NaN maps to the first sample instead of propagating.
    let q = if q.is_nan() { 0.0 } else { q.clamp(0.0, 1.0) };
    let exact = q * n as f64;
    // `q * n` can land a few ULPs above an integer (0.07 * 100 is
    // 7.000000000000001), which would push `ceil` one rank too high.
    // Snap values that are within rounding error of an integer.
    let rounded = exact.round();
    let rank = if (exact - rounded).abs() <= exact.abs() * 4.0 * f64::EPSILON {
        rounded
    } else {
        exact.ceil()
    };
    let rank = (rank as usize).clamp(1, n);
    rank - 1
}

/// Nearest-rank percentile of an already sorted slice. `Duration::ZERO`
/// when the slice is empty.
fn nearest_rank(sorted: &[Duration], q: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    sorted[nearest_rank_index(sorted.len(), q)]
}

fn compute_cv(samples: &[Duration], mean: Duration) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mean_s = mean.as_secs_f64();
    if mean_s == 0.0 {
        return 0.0;
    }
    let n = samples.len() as f64;
    let var = samples
        .iter()
        .map(|d| (d.as_secs_f64() - mean_s).powi(2))
        .sum::<f64>()
        / n;
    var.sqrt() / mean_s
}

/// One bin of a sample-distribution histogram.
///
/// Returned by [`BenchmarkResult::histogram`]. Bins are ordered, the
/// first bin's `lower` equals `BenchmarkResult::min` and the last
/// bin's `upper` equals `BenchmarkResult::max`.
///
/// # Example
///
/// ```
/// use dev_bench::Benchmark;
///
/// let mut b = Benchmark::new("h");
/// for _ in 0..10 { b.iter(|| std::hint::black_box(1 + 1)); }
/// let bins = b.finish().histogram(4);
/// assert!(bins.iter().all(|b| b.lower <= b.upper));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistogramBin {
    /// Inclusive lower bound of this bin.
    pub lower: Duration,
    /// Inclusive upper bound (for the last bin) or exclusive upper
    /// bound (for all other bins).
    pub upper: Duration,
    /// Number of samples falling into this bin.
    pub count: usize,
}

/// The result of a finished benchmark.
///
/// Statistics are computed losslessly from the raw `samples`.
///
/// # Example
///
/// ```
/// use dev_bench::Benchmark;
///
/// let mut b = Benchmark::new("noop");
/// for _ in 0..10 {
///     b.iter(|| std::hint::black_box(42));
/// }
/// let r = b.finish();
/// assert!(r.mean.as_nanos() > 0);
/// ```
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    /// Stable name of the benchmark.
    pub name: String,
    /// All raw sample durations.
    pub samples: Vec<Duration>,
    /// Total iterations across all samples. With per-iter sampling this
    /// equals `samples.len()`. With batched sampling, it is the sum of
    /// `n` across all `iter_with_count` calls.
    pub iterations_recorded: u64,
    /// Sum of all sample durations.
    pub total_elapsed: Duration,
    /// Mean sample duration.
    pub mean: Duration,
    /// 50th percentile sample duration (nearest-rank, see
    /// [`percentile`](BenchmarkResult::percentile)).
    pub p50: Duration,
    /// 99th percentile sample duration (nearest-rank, see
    /// [`percentile`](BenchmarkResult::percentile)).
    pub p99: Duration,
    /// Coefficient of variation across samples (stddev / mean).
    ///
    /// The standard deviation here is the population form (divides by
    /// `n`), so it can be slightly smaller than [`stddev`](BenchmarkResult::stddev),
    /// which divides by `n - 1`. `0.0` for an empty result or a zero mean.
    ///
    /// Higher numbers indicate noisier measurements. A CV of `0.05`
    /// means the standard deviation is 5% of the mean. Reported
    /// regressions within the CV are downgraded from `Fail` to `Warn`
    /// by [`compare_with_options`](BenchmarkResult::compare_with_options).
    pub cv: f64,
}

impl BenchmarkResult {
    /// Effective throughput in operations per second.
    ///
    /// Defined as `iterations_recorded / total_elapsed_seconds`. Returns
    /// `0.0` for an empty result or zero elapsed time.
    ///
    /// # Example
    ///
    /// ```
    /// use dev_bench::Benchmark;
    ///
    /// let mut b = Benchmark::new("hot");
    /// b.iter_with_count(1000, || { std::hint::black_box(1 + 1); });
    /// let r = b.finish();
    /// assert!(r.ops_per_sec() > 0.0);
    /// ```
    pub fn ops_per_sec(&self) -> f64 {
        if self.total_elapsed.is_zero() {
            return 0.0;
        }
        self.iterations_recorded as f64 / self.total_elapsed.as_secs_f64()
    }

    /// Smallest sample. Returns `Duration::ZERO` for an empty result.
    pub fn min(&self) -> Duration {
        self.samples.iter().copied().min().unwrap_or(Duration::ZERO)
    }

    /// Largest sample. Returns `Duration::ZERO` for an empty result.
    pub fn max(&self) -> Duration {
        self.samples.iter().copied().max().unwrap_or(Duration::ZERO)
    }

    /// Sample standard deviation, in seconds. `0.0` for fewer than 2 samples.
    ///
    /// Uses `n-1` (Bessel's correction) for the sample variance.
    pub fn stddev(&self) -> f64 {
        let n = self.samples.len();
        if n < 2 {
            return 0.0;
        }
        let mean_s = self.mean.as_secs_f64();
        let var = self
            .samples
            .iter()
            .map(|d| (d.as_secs_f64() - mean_s).powi(2))
            .sum::<f64>()
            / (n as f64 - 1.0);
        var.sqrt()
    }

    /// Median absolute deviation, in seconds. `0.0` for empty results.
    ///
    /// `MAD = median(|x_i - median(x)|)`, where `median(x)` is `p50` and
    /// both medians use the same nearest-rank rule as
    /// [`percentile`](BenchmarkResult::percentile). Less affected by
    /// outliers than standard deviation; useful for noisy measurements.
    pub fn mad(&self) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let p50_s = self.p50.as_secs_f64();
        let mut deviations: Vec<f64> = self
            .samples
            .iter()
            .map(|d| (d.as_secs_f64() - p50_s).abs())
            .collect();
        deviations.sort_by(f64::total_cmp);
        deviations[nearest_rank_index(deviations.len(), 0.5)]
    }

    /// 90th percentile sample duration. `Duration::ZERO` for empty results.
    pub fn p90(&self) -> Duration {
        self.percentile(0.90)
    }

    /// 99.9th percentile sample duration. `Duration::ZERO` for empty results.
    ///
    /// At least 1000 samples are required to be meaningful; with fewer
    /// samples this returns the largest sample.
    pub fn p999(&self) -> Duration {
        self.percentile(0.999)
    }

    /// Compute an arbitrary percentile (0.0..=1.0). Returns `Duration::ZERO`
    /// for empty results. Uses nearest-rank, the same as `p50`/`p99`.
    ///
    /// Nearest-rank picks the sample at rank `ceil(q * n)` (1-based) in
    /// sorted order, so the result is always one of the recorded samples.
    /// For 100 samples, `p99` is the 99th smallest and `p50` the 50th.
    /// `q` is clamped to `0.0..=1.0`; `q <= 0.0` and NaN give the smallest
    /// sample, `q >= 1.0` the largest.
    ///
    /// # Example
    ///
    /// ```
    /// use dev_bench::BenchmarkResult;
    /// use std::time::Duration;
    ///
    /// let samples: Vec<Duration> = (1..=100).map(Duration::from_nanos).collect();
    /// let r = BenchmarkResult {
    ///     name: "ranks".into(),
    ///     samples,
    ///     iterations_recorded: 100,
    ///     total_elapsed: Duration::from_nanos(5050),
    ///     mean: Duration::from_nanos(50),
    ///     p50: Duration::from_nanos(50),
    ///     p99: Duration::from_nanos(99),
    ///     cv: 0.0,
    /// };
    /// assert_eq!(r.percentile(0.99), Duration::from_nanos(99));
    /// assert_eq!(r.percentile(0.50), Duration::from_nanos(50));
    /// assert_eq!(r.percentile(1.0), Duration::from_nanos(100));
    /// ```
    pub fn percentile(&self, q: f64) -> Duration {
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        nearest_rank(&sorted, q)
    }

    /// Compute a uniform-width histogram over the sample distribution.
    ///
    /// Returns `bucket_count` bins covering `[min, max]`, each with
    /// the count of samples falling into that bin. The returned
    /// `Vec<HistogramBin>` is in ascending order; the first bin's
    /// `lower` equals `min()`, the last bin's `upper` equals `max()`.
    ///
    /// For an empty result or `bucket_count == 0`, returns `vec![]`.
    /// When `min == max` (all samples equal), returns one bin with
    /// the full sample count.
    ///
    /// Useful for spotting bimodality, outlier tails, and warmup
    /// effects that mean/percentile alone hide.
    ///
    /// # Example
    ///
    /// ```
    /// use dev_bench::Benchmark;
    ///
    /// let mut b = Benchmark::new("h");
    /// for _ in 0..50 { b.iter(|| std::hint::black_box(1 + 1)); }
    /// let r = b.finish();
    /// let hist = r.histogram(8);
    /// assert!(hist.len() <= 8);
    /// let total: usize = hist.iter().map(|h| h.count).sum();
    /// assert_eq!(total, r.samples.len());
    /// ```
    pub fn histogram(&self, bucket_count: usize) -> Vec<HistogramBin> {
        if bucket_count == 0 || self.samples.is_empty() {
            return Vec::new();
        }
        let min = self.min();
        let max = self.max();
        if min == max {
            return vec![HistogramBin {
                lower: min,
                upper: max,
                count: self.samples.len(),
            }];
        }
        let total_ns = (max.as_nanos() - min.as_nanos()) as f64;
        let bucket_ns = total_ns / bucket_count as f64;
        let mut counts = vec![0usize; bucket_count];
        for s in &self.samples {
            let offset = (s.as_nanos() - min.as_nanos()) as f64;
            let mut idx = (offset / bucket_ns).floor() as usize;
            if idx >= bucket_count {
                idx = bucket_count - 1;
            }
            counts[idx] += 1;
        }
        let min_ns = min.as_nanos() as u64;
        let mut bins = Vec::with_capacity(bucket_count);
        for (i, count) in counts.into_iter().enumerate() {
            let lower_ns = min_ns + (bucket_ns * i as f64) as u64;
            let upper_ns = if i + 1 == bucket_count {
                max.as_nanos() as u64
            } else {
                min_ns + (bucket_ns * (i + 1) as f64) as u64
            };
            bins.push(HistogramBin {
                lower: Duration::from_nanos(lower_ns),
                upper: Duration::from_nanos(upper_ns),
                count,
            });
        }
        bins
    }

    /// Compare this result against a baseline using a default-tuned
    /// [`CompareOptions`].
    ///
    /// `baseline_mean` is the previous mean duration. If `None`, the
    /// verdict is `Skip` and no comparison is made.
    ///
    /// # Example
    ///
    /// ```
    /// use dev_bench::{Benchmark, Threshold};
    /// use std::time::Duration;
    ///
    /// let mut b = Benchmark::new("x");
    /// b.iter(|| std::hint::black_box(1 + 1));
    /// let r = b.finish();
    /// let _ = r.compare_against_baseline(
    ///     Some(Duration::from_nanos(1)),
    ///     Threshold::regression_pct(10.0),
    /// );
    /// ```
    pub fn compare_against_baseline(
        &self,
        baseline_mean: Option<Duration>,
        threshold: Threshold,
    ) -> CheckResult {
        self.compare_with_options(&CompareOptions {
            baseline_mean,
            threshold,
            ..CompareOptions::default()
        })
    }

    /// Compare this result against a baseline using full options.
    ///
    /// Behavior:
    /// - No baseline -> `Skip`.
    /// - Sample count below `min_samples` -> `Skip` with detail.
    /// - Percent threshold (`RegressionPct`, `ThroughputDropPct`) with a
    ///   non-finite percent, or a zero baseline mean while the current
    ///   mean is non-zero -> `Skip` with detail. (Both means zero, as
    ///   happens below the timer's resolution, is a `Pass`.)
    /// - Within threshold -> `Pass` with numeric evidence.
    /// - Over threshold but within CV noise band -> `Warn`.
    /// - Over threshold and outside CV noise band -> `Fail (Warning)`.
    ///
    /// In every non-`Skip` case, the returned [`CheckResult`] carries
    /// a `bench` tag and numeric `Evidence` for `mean_ns`,
    /// `baseline_ns`, `p50_ns`, `p99_ns`, `cv`, and `ops_per_sec`.
    ///
    /// # Example
    ///
    /// ```
    /// use dev_bench::{Benchmark, CompareOptions, Threshold};
    /// use std::time::Duration;
    ///
    /// let mut b = Benchmark::new("x");
    /// b.iter(|| std::hint::black_box(1 + 1));
    /// let r = b.finish();
    /// let opts = CompareOptions {
    ///     baseline_mean: Some(Duration::from_nanos(1)),
    ///     threshold: Threshold::regression_pct(20.0),
    ///     min_samples: 1,
    ///     allow_cv_noise_band: true,
    /// };
    /// let _check = r.compare_with_options(&opts);
    /// ```
    pub fn compare_with_options(&self, opts: &CompareOptions) -> CheckResult {
        let name = format!("bench::{}", self.name);
        let mut evidence = self.numeric_evidence();
        let tags = vec!["bench".to_string()];

        let Some(baseline) = opts.baseline_mean else {
            let mut c = CheckResult::skip(name).with_detail("no baseline available");
            c.tags = tags;
            c.evidence = evidence;
            return c;
        };

        if (self.samples.len() as u64) < opts.min_samples {
            let mut c = CheckResult::skip(name).with_detail(format!(
                "fewer samples than min_samples ({} < {})",
                self.samples.len(),
                opts.min_samples
            ));
            c.tags = tags;
            c.evidence = evidence;
            return c;
        }

        let current_ns = self.mean.as_nanos();
        let baseline_ns = baseline.as_nanos();
        evidence.insert(1, Evidence::numeric("baseline_ns", baseline_ns as f64));

        // A percent change from a zero baseline (a run below the timer's
        // resolution) is undefined: any non-zero mean would count as an
        // infinite regression, and a zero throughput floor never trips. Two
        // zero means are simply equal and fall through to `Pass`. A NaN or
        // infinite percent would silently pass every run. Report the
        // undefined cases as `Skip` so they are visible instead of giving
        // a verdict.
        if let Threshold::RegressionPct(pct) | Threshold::ThroughputDropPct(pct) = opts.threshold {
            let reason = if !pct.is_finite() {
                Some(format!("threshold percent is not a finite number ({pct})"))
            } else if baseline.is_zero() && current_ns != 0 {
                Some("baseline mean is zero; a percent threshold cannot be applied".to_string())
            } else {
                None
            };
            if let Some(reason) = reason {
                let mut c = CheckResult::skip(name).with_detail(reason);
                c.tags = tags;
                c.evidence = evidence;
                return c;
            }
        }

        let regressed = match opts.threshold {
            Threshold::RegressionPct(pct) => {
                let allowed = baseline_ns as f64 * (1.0 + pct / 100.0);
                current_ns as f64 > allowed
            }
            Threshold::RegressionAbsoluteNs(abs) => current_ns.saturating_sub(baseline_ns) > abs,
            Threshold::ThroughputDropPct(pct) => {
                // Throughput-based; convert via mean.
                let baseline_ops = if baseline.is_zero() {
                    0.0
                } else {
                    1.0 / baseline.as_secs_f64()
                };
                let drop_floor = baseline_ops * (1.0 - pct / 100.0);
                self.ops_per_sec() < drop_floor
            }
        };

        let detail = format!(
            "current_ns={} baseline_ns={} cv={:.4} ops/sec={:.0}",
            current_ns,
            baseline_ns,
            self.cv,
            self.ops_per_sec()
        );

        if !regressed {
            let mut c = CheckResult::pass(name).with_detail(detail);
            c.tags = tags;
            c.evidence = evidence;
            return c;
        }

        // Regression detected. Decide Fail vs Warn based on CV noise band.
        let in_noise_band = opts.allow_cv_noise_band && {
            let allowed_noise_ns = baseline_ns as f64 * self.cv;
            let delta_ns = (current_ns as f64) - (baseline_ns as f64);
            delta_ns <= allowed_noise_ns
        };
        let mut tags = tags;
        tags.push("regression".to_string());
        if in_noise_band {
            let mut c = CheckResult::warn(name, Severity::Warning)
                .with_detail(format!("{} (within CV noise band)", detail));
            c.tags = tags;
            c.evidence = evidence;
            c
        } else {
            let mut c = CheckResult::fail(name, Severity::Warning).with_detail(detail);
            c.tags = tags;
            c.evidence = evidence;
            c
        }
    }

    /// Build a one-check `Report` containing the comparison result.
    ///
    /// Convenience for producers that want a complete `Report` rather
    /// than a single `CheckResult`. Sets `subject = self.name`,
    /// `producer = "dev-bench"`.
    ///
    /// # Example
    ///
    /// ```
    /// use dev_bench::{Benchmark, Threshold};
    ///
    /// let mut b = Benchmark::new("x");
    /// b.iter(|| std::hint::black_box(1 + 1));
    /// let r = b.finish();
    /// let report = r.into_report("0.1.0", None, Threshold::regression_pct(10.0));
    /// assert_eq!(report.checks.len(), 1);
    /// ```
    pub fn into_report(
        self,
        subject_version: impl Into<String>,
        baseline_mean: Option<Duration>,
        threshold: Threshold,
    ) -> Report {
        let name = self.name.clone();
        let check = self.compare_against_baseline(baseline_mean, threshold);
        let mut r = Report::new(name, subject_version).with_producer("dev-bench");
        r.push(check);
        r.finish();
        r
    }

    fn numeric_evidence(&self) -> Vec<Evidence> {
        vec![
            Evidence::numeric("mean_ns", self.mean.as_nanos() as f64),
            // baseline_ns inserted at index 1 by callers when available.
            Evidence::numeric("p50_ns", self.p50.as_nanos() as f64),
            Evidence::numeric("p99_ns", self.p99.as_nanos() as f64),
            Evidence::numeric("cv", self.cv),
            Evidence::numeric("ops_per_sec", self.ops_per_sec()),
            Evidence::numeric("samples", self.samples.len() as f64),
            Evidence::numeric("iterations_recorded", self.iterations_recorded as f64),
        ]
    }
}

/// A threshold defining how much slower-than-baseline is acceptable.
#[derive(Debug, Clone, Copy)]
pub enum Threshold {
    /// Fail if the new mean is more than `pct` percent slower than baseline.
    RegressionPct(f64),
    /// Fail if `current_mean - baseline_mean` exceeds `nanos`.
    RegressionAbsoluteNs(u128),
    /// Fail if throughput dropped more than `pct` percent below baseline.
    ///
    /// Baseline ops/sec is derived as `1.0 / baseline_mean_secs`, which
    /// assumes the baseline duration is a per-operation duration. Use
    /// with per-iter sampling (`Benchmark::iter`) where `mean` equals
    /// per-op duration. For batched sampling
    /// (`Benchmark::iter_with_count`), prefer a duration-based threshold
    /// or pre-compute the baseline manually.
    ThroughputDropPct(f64),
}

impl Threshold {
    /// Build a percent-based duration regression threshold.
    ///
    /// # Example
    ///
    /// ```
    /// use dev_bench::Threshold;
    /// let t = Threshold::regression_pct(20.0);
    /// assert!(matches!(t, Threshold::RegressionPct(_)));
    /// ```
    pub fn regression_pct(pct: f64) -> Self {
        Threshold::RegressionPct(pct)
    }

    /// Build an absolute duration regression threshold in nanoseconds.
    ///
    /// # Example
    ///
    /// ```
    /// use dev_bench::Threshold;
    /// let t = Threshold::regression_abs_ns(500);
    /// assert!(matches!(t, Threshold::RegressionAbsoluteNs(_)));
    /// ```
    pub fn regression_abs_ns(nanos: u128) -> Self {
        Threshold::RegressionAbsoluteNs(nanos)
    }

    /// Build a percent-based throughput drop threshold.
    ///
    /// # Example
    ///
    /// ```
    /// use dev_bench::Threshold;
    /// let t = Threshold::throughput_drop_pct(10.0);
    /// assert!(matches!(t, Threshold::ThroughputDropPct(_)));
    /// ```
    pub fn throughput_drop_pct(pct: f64) -> Self {
        Threshold::ThroughputDropPct(pct)
    }
}

/// Options for [`BenchmarkResult::compare_with_options`].
///
/// Defaults: no baseline, percent threshold of 10%, `min_samples = 1`,
/// `allow_cv_noise_band = true`.
///
/// # Example
///
/// ```
/// use dev_bench::{CompareOptions, Threshold};
/// use std::time::Duration;
///
/// let opts = CompareOptions {
///     baseline_mean: Some(Duration::from_nanos(1000)),
///     threshold: Threshold::regression_pct(20.0),
///     min_samples: 30,
///     allow_cv_noise_band: true,
/// };
/// assert_eq!(opts.min_samples, 30);
/// ```
#[derive(Debug, Clone)]
pub struct CompareOptions {
    /// Baseline mean to compare against. `None` -> verdict is `Skip`.
    pub baseline_mean: Option<Duration>,
    /// Regression threshold to apply.
    pub threshold: Threshold,
    /// Minimum sample count required before a comparison can be made.
    /// Below this, the verdict is `Skip` with a `min_samples` detail.
    pub min_samples: u64,
    /// If `true`, regressions within `baseline_ns * cv` are downgraded
    /// from `Fail` to `Warn`.
    pub allow_cv_noise_band: bool,
}

impl Default for CompareOptions {
    fn default() -> Self {
        Self {
            baseline_mean: None,
            threshold: Threshold::regression_pct(10.0),
            min_samples: 1,
            allow_cv_noise_band: true,
        }
    }
}

/// A trait for any object that can run a benchmark and produce a result.
pub trait Bench {
    /// Run the benchmark and return its result.
    fn run(&mut self) -> BenchmarkResult;
}

/// Producer wrapper that runs a benchmark and emits a single-check
/// [`Report`] via [`Producer::produce`].
///
/// # Example
///
/// ```no_run
/// use dev_bench::{Benchmark, BenchProducer, Threshold};
/// use dev_report::Producer;
///
/// fn run_bench() -> dev_bench::BenchmarkResult {
///     let mut b = Benchmark::new("hot_path");
///     for _ in 0..10 { b.iter(|| std::hint::black_box(1 + 1)); }
///     b.finish()
/// }
///
/// let producer = BenchProducer::new(run_bench, "0.1.0", None, Threshold::regression_pct(10.0));
/// let report = producer.produce();
/// assert_eq!(report.checks.len(), 1);
/// ```
pub struct BenchProducer<F>
where
    F: Fn() -> BenchmarkResult,
{
    run: F,
    subject_version: String,
    baseline_mean: Option<Duration>,
    threshold: Threshold,
}

impl<F> BenchProducer<F>
where
    F: Fn() -> BenchmarkResult,
{
    /// Build a new producer.
    pub fn new(
        run: F,
        subject_version: impl Into<String>,
        baseline_mean: Option<Duration>,
        threshold: Threshold,
    ) -> Self {
        Self {
            run,
            subject_version: subject_version.into(),
            baseline_mean,
            threshold,
        }
    }
}

impl<F> Producer for BenchProducer<F>
where
    F: Fn() -> BenchmarkResult,
{
    fn produce(&self) -> Report {
        let result = (self.run)();
        result.into_report(
            self.subject_version.clone(),
            self.baseline_mean,
            self.threshold,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dev_report::Verdict;

    #[test]
    fn benchmark_runs_and_finishes() {
        let mut b = Benchmark::new("noop");
        for _ in 0..10 {
            b.iter(|| std::hint::black_box(42));
        }
        let r = b.finish();
        assert_eq!(r.samples.len(), 10);
        assert_eq!(r.iterations_recorded, 10);
        assert!(r.mean > Duration::ZERO);
    }

    #[test]
    fn iter_with_count_records_one_sample() {
        let mut b = Benchmark::new("hot");
        b.iter_with_count(1000, || {
            std::hint::black_box(1 + 1);
        });
        let r = b.finish();
        assert_eq!(r.samples.len(), 1);
        assert_eq!(r.iterations_recorded, 1000);
        assert!(r.ops_per_sec() > 0.0);
    }

    #[test]
    fn comparison_without_baseline_is_skip() {
        let mut b = Benchmark::new("x");
        b.iter(|| ());
        let r = b.finish();
        let v = r.compare_against_baseline(None, Threshold::regression_pct(5.0));
        assert_eq!(v.verdict, Verdict::Skip);
        assert!(v.has_tag("bench"));
    }

    #[test]
    fn min_samples_skip() {
        let mut b = Benchmark::new("x");
        b.iter(|| ());
        let r = b.finish();
        let opts = CompareOptions {
            baseline_mean: Some(Duration::from_nanos(100)),
            threshold: Threshold::regression_pct(5.0),
            min_samples: 100,
            allow_cv_noise_band: true,
        };
        let v = r.compare_with_options(&opts);
        assert_eq!(v.verdict, Verdict::Skip);
        assert!(v.detail.unwrap().contains("min_samples"));
    }

    #[test]
    fn small_regression_under_threshold_passes() {
        let mut b = Benchmark::new("x");
        for _ in 0..5 {
            b.iter(|| std::thread::sleep(Duration::from_micros(1)));
        }
        let r = b.finish();
        let baseline = r.mean;
        let v = r.compare_against_baseline(Some(baseline), Threshold::regression_pct(50.0));
        assert_eq!(v.verdict, Verdict::Pass);
        assert!(v.has_tag("bench"));
        // Numeric evidence is attached.
        assert!(v.evidence.iter().any(|e| e.label == "mean_ns"));
        assert!(v.evidence.iter().any(|e| e.label == "baseline_ns"));
        assert!(v.evidence.iter().any(|e| e.label == "ops_per_sec"));
    }

    #[test]
    fn regression_outside_cv_band_fails() {
        // Baseline 100ns, current 200ns, threshold 10%, cv ~0.
        let mut b = Benchmark::new("x");
        // Inject controlled samples by running noop iterations.
        for _ in 0..50 {
            b.iter(|| std::hint::black_box(1 + 1));
        }
        let mut r = b.finish();
        // Force a known mean and cv for deterministic comparison.
        r.mean = Duration::from_nanos(200);
        r.cv = 0.0;
        let opts = CompareOptions {
            baseline_mean: Some(Duration::from_nanos(100)),
            threshold: Threshold::regression_pct(10.0),
            min_samples: 1,
            allow_cv_noise_band: true,
        };
        let v = r.compare_with_options(&opts);
        assert_eq!(v.verdict, Verdict::Fail);
        assert!(v.has_tag("regression"));
    }

    #[test]
    fn regression_inside_cv_band_warns() {
        let mut b = Benchmark::new("x");
        for _ in 0..50 {
            b.iter(|| std::hint::black_box(1 + 1));
        }
        let mut r = b.finish();
        // Current is 12% over baseline but cv is 30% -> within noise band.
        r.mean = Duration::from_nanos(112);
        r.cv = 0.30;
        let opts = CompareOptions {
            baseline_mean: Some(Duration::from_nanos(100)),
            threshold: Threshold::regression_pct(10.0),
            min_samples: 1,
            allow_cv_noise_band: true,
        };
        let v = r.compare_with_options(&opts);
        assert_eq!(v.verdict, Verdict::Warn);
        assert!(v.has_tag("regression"));
        assert!(v.detail.unwrap().contains("CV noise band"));
    }

    #[test]
    fn throughput_threshold_detects_drop() {
        // ThroughputDropPct expects a per-op baseline duration. Use
        // per-iter sampling so mean == per-op duration.
        let mut b = Benchmark::new("x");
        for _ in 0..10 {
            b.iter(|| std::thread::sleep(Duration::from_micros(1)));
        }
        let r = b.finish();
        // Baseline 10x faster (per-op duration is 1/10 of current);
        // current throughput is 90% lower than baseline -> regression.
        let baseline = r.mean / 10;
        let v = r.compare_against_baseline(Some(baseline), Threshold::throughput_drop_pct(50.0));
        assert_eq!(v.verdict, Verdict::Fail);
    }

    #[test]
    fn extra_stats_are_consistent() {
        let mut b = Benchmark::new("uniform");
        for _ in 0..20 {
            b.iter(|| std::hint::black_box(1 + 1));
        }
        let r = b.finish();
        // Bounds.
        assert!(r.min() <= r.mean);
        assert!(r.mean <= r.max());
        assert!(r.p50 <= r.p90());
        assert!(r.p90() <= r.p99);
        assert!(r.p99 <= r.p999());
        // Numbers are non-negative finite.
        assert!(r.stddev() >= 0.0);
        assert!(r.mad() >= 0.0);
    }

    #[test]
    fn percentile_clamps_to_bounds() {
        let mut b = Benchmark::new("p");
        for _ in 0..10 {
            b.iter(|| std::hint::black_box(1));
        }
        let r = b.finish();
        // q < 0.0 -> first sample; q > 1.0 -> last sample.
        let lo = r.percentile(-0.5);
        let hi = r.percentile(1.5);
        assert!(lo <= hi);
    }

    #[test]
    fn empty_result_stats_are_zero() {
        let r = Benchmark::new("empty").finish();
        assert_eq!(r.min(), Duration::ZERO);
        assert_eq!(r.max(), Duration::ZERO);
        assert_eq!(r.p90(), Duration::ZERO);
        assert_eq!(r.p999(), Duration::ZERO);
        assert_eq!(r.stddev(), 0.0);
        assert_eq!(r.mad(), 0.0);
    }

    #[test]
    fn run_for_collects_at_least_one_sample() {
        let mut b = Benchmark::new("budget");
        b.run_for(Duration::from_millis(10), || {
            std::hint::black_box(1 + 1);
        });
        let r = b.finish();
        assert!(!r.samples.is_empty());
        assert_eq!(r.iterations_recorded, r.samples.len() as u64);
    }

    #[test]
    fn run_for_zero_budget_collects_no_samples() {
        let mut b = Benchmark::new("zero");
        b.run_for(Duration::ZERO, || {
            std::hint::black_box(1 + 1);
        });
        let r = b.finish();
        // With zero budget, deadline has already passed; no iterations.
        assert!(r.samples.is_empty() || r.samples.len() <= 1);
    }

    #[test]
    fn histogram_total_count_equals_samples() {
        let mut b = Benchmark::new("h");
        for _ in 0..50 {
            b.iter(|| std::hint::black_box(1 + 1));
        }
        let r = b.finish();
        let bins = r.histogram(8);
        assert!(!bins.is_empty());
        let total: usize = bins.iter().map(|b| b.count).sum();
        assert_eq!(total, r.samples.len());
    }

    #[test]
    fn histogram_zero_buckets_returns_empty() {
        let mut b = Benchmark::new("h");
        b.iter(|| std::hint::black_box(1));
        let r = b.finish();
        assert!(r.histogram(0).is_empty());
    }

    #[test]
    fn histogram_empty_result_returns_empty() {
        let r = Benchmark::new("e").finish();
        assert!(r.histogram(8).is_empty());
    }

    #[test]
    fn histogram_bins_are_ordered() {
        let mut b = Benchmark::new("h");
        for _ in 0..30 {
            b.iter(|| std::hint::black_box(1 + 1));
        }
        let bins = b.finish().histogram(5);
        for win in bins.windows(2) {
            assert!(win[0].lower <= win[1].lower);
            assert!(win[0].lower <= win[0].upper);
        }
    }

    #[test]
    fn cv_is_zero_for_uniform_samples() {
        // Samples are nearly identical -> cv near 0.
        let mut b = Benchmark::new("x");
        for _ in 0..10 {
            b.iter(|| std::hint::black_box(1 + 1));
        }
        let r = b.finish();
        // Not strictly zero on real machines, just bounded.
        assert!(r.cv >= 0.0);
    }

    #[test]
    fn into_report_emits_one_check() {
        let mut b = Benchmark::new("x");
        for _ in 0..5 {
            b.iter(|| std::hint::black_box(1 + 1));
        }
        let r = b.finish();
        let baseline = r.mean;
        let report = r.into_report("0.1.0", Some(baseline), Threshold::regression_pct(50.0));
        assert_eq!(report.checks.len(), 1);
        assert_eq!(report.producer.as_deref(), Some("dev-bench"));
        assert_eq!(report.overall_verdict(), Verdict::Pass);
    }

    fn bench_with_nanos(nanos: &[u64]) -> Benchmark {
        Benchmark {
            name: "fixed".into(),
            samples: nanos.iter().copied().map(Duration::from_nanos).collect(),
            iterations_recorded: nanos.len() as u64,
        }
    }

    #[test]
    fn finish_uses_nearest_rank_percentiles() {
        // 1..=100 ns in shuffled order. Nearest-rank p50 is the 50th
        // smallest and p99 the 99th smallest. The old `floor(n * q)`
        // index returned 51 and 100.
        let mut nanos: Vec<u64> = (1..=100).collect();
        nanos.reverse();
        let r = bench_with_nanos(&nanos).finish();
        assert_eq!(r.p50, Duration::from_nanos(50));
        assert_eq!(r.p99, Duration::from_nanos(99));
        assert_eq!(r.p90(), Duration::from_nanos(90));
        assert_eq!(r.p999(), Duration::from_nanos(100));
        assert_eq!(r.percentile(0.0), Duration::from_nanos(1));
        assert_eq!(r.percentile(f64::NAN), Duration::from_nanos(1));
        assert_eq!(r.percentile(1.0), Duration::from_nanos(100));
        // 0.07 * 100 is 7.000000000000001 in f64; rank must still be 7.
        assert_eq!(r.percentile(0.07), Duration::from_nanos(7));
        assert_eq!(r.mean, Duration::from_nanos(50)); // 5050 / 100, truncated
        assert_eq!(r.total_elapsed, Duration::from_nanos(5050));
    }

    #[test]
    fn single_sample_stats_equal_that_sample() {
        let r = bench_with_nanos(&[700]).finish();
        let s = Duration::from_nanos(700);
        assert_eq!(r.mean, s);
        assert_eq!(r.p50, s);
        assert_eq!(r.p99, s);
        assert_eq!(r.p90(), s);
        assert_eq!(r.min(), s);
        assert_eq!(r.max(), s);
        assert_eq!(r.cv, 0.0);
        assert_eq!(r.stddev(), 0.0);
        assert_eq!(r.mad(), 0.0);
        assert_eq!(r.histogram(4).len(), 1);
    }

    #[test]
    fn two_samples_median_is_lower_sample() {
        let r = bench_with_nanos(&[300, 100]).finish();
        assert_eq!(r.p50, Duration::from_nanos(100));
        assert_eq!(r.p99, Duration::from_nanos(300));
        assert_eq!(r.mean, Duration::from_nanos(200));
    }

    #[test]
    fn empty_finish_is_all_zero() {
        let r = bench_with_nanos(&[]).finish();
        assert_eq!(r.mean, Duration::ZERO);
        assert_eq!(r.p50, Duration::ZERO);
        assert_eq!(r.p99, Duration::ZERO);
        assert_eq!(r.cv, 0.0);
        assert_eq!(r.ops_per_sec(), 0.0);
    }

    #[test]
    fn mad_matches_hand_computed_value() {
        // Sorted: 10 20 30 40 1000. Median 30. Deviations: 20 10 0 10 970
        // -> sorted 0 10 10 20 970 -> median 10 ns.
        let r = bench_with_nanos(&[1000, 10, 40, 20, 30]).finish();
        assert_eq!(r.p50, Duration::from_nanos(30));
        assert!((r.mad() - 10e-9).abs() < 1e-15);
    }

    #[test]
    fn cv_and_stddev_hand_computed() {
        // Samples 100 and 300: mean 200, population sd 100, sample sd ~141.42.
        let r = bench_with_nanos(&[100, 300]).finish();
        assert!((r.cv - 0.5).abs() < 1e-12);
        assert!((r.stddev() - 141.421_356_237e-9).abs() < 1e-15);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn mean_handles_more_than_u32_max_samples() {
        // `Duration / u32` truncated the count; exactly 2^32 samples
        // became a division by zero.
        let n = 1usize << 32;
        let total = Duration::from_nanos(10 * (1u64 << 32));
        assert_eq!(mean_of(total, n), Duration::from_nanos(10));
        assert_eq!(mean_of(Duration::from_secs(3), 0), Duration::ZERO);
        assert_eq!(
            mean_of(Duration::new(5, 500_000_000), 2),
            Duration::new(2, 750_000_000)
        );
    }

    #[test]
    fn run_for_with_huge_budget_does_not_overflow() {
        // `Instant::now() + Duration::MAX` used to panic before the first
        // iteration. The closure panics on its third call to end the run.
        let mut calls = 0u32;
        let mut b = Benchmark::new("huge");
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            b.run_for(Duration::MAX, || {
                calls += 1;
                if calls == 3 {
                    panic!("stop");
                }
            });
        }));
        assert!(outcome.is_err());
        assert_eq!(calls, 3);
    }

    #[test]
    fn zero_baseline_with_percent_threshold_skips() {
        let r = bench_with_nanos(&[100, 100]).finish();
        for t in [
            Threshold::regression_pct(10.0),
            Threshold::throughput_drop_pct(10.0),
        ] {
            let v = r.compare_against_baseline(Some(Duration::ZERO), t);
            assert_eq!(v.verdict, Verdict::Skip, "{t:?}");
            assert!(v
                .detail
                .as_deref()
                .unwrap()
                .contains("baseline mean is zero"));
            assert!(v.evidence.iter().any(|e| e.label == "baseline_ns"));
        }
        // An absolute threshold still works against a zero baseline.
        let v = r.compare_against_baseline(Some(Duration::ZERO), Threshold::regression_abs_ns(50));
        assert_eq!(v.verdict, Verdict::Fail);
        // Zero against zero (both below timer resolution) is equal: Pass.
        let zero = bench_with_nanos(&[0, 0, 0]).finish();
        for t in [
            Threshold::regression_pct(10.0),
            Threshold::throughput_drop_pct(10.0),
        ] {
            let v = zero.compare_against_baseline(Some(Duration::ZERO), t);
            assert_eq!(v.verdict, Verdict::Pass, "{t:?}");
        }
    }

    #[test]
    fn non_finite_percent_threshold_skips() {
        let r = bench_with_nanos(&[500, 500]).finish();
        for pct in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let v = r.compare_against_baseline(
                Some(Duration::from_nanos(100)),
                Threshold::regression_pct(pct),
            );
            assert_eq!(v.verdict, Verdict::Skip, "pct={pct}");
            assert!(v.detail.as_deref().unwrap().contains("not a finite number"));
        }
    }

    #[test]
    fn regression_pct_boundary_is_inclusive_pass() {
        // Exactly +10% over a 100 ns baseline passes; one more ns fails.
        let at = bench_with_nanos(&[110]).finish();
        let over = bench_with_nanos(&[111]).finish();
        let opts = |r: &BenchmarkResult| {
            r.compare_with_options(&CompareOptions {
                baseline_mean: Some(Duration::from_nanos(100)),
                threshold: Threshold::regression_pct(10.0),
                min_samples: 1,
                allow_cv_noise_band: false,
            })
        };
        assert_eq!(opts(&at).verdict, Verdict::Pass);
        assert_eq!(opts(&over).verdict, Verdict::Fail);
    }

    #[test]
    fn bench_producer_implements_producer_trait() {
        fn run() -> BenchmarkResult {
            let mut b = Benchmark::new("noop");
            for _ in 0..5 {
                b.iter(|| std::hint::black_box(1 + 1));
            }
            b.finish()
        }
        let p = BenchProducer::new(run, "0.1.0", None, Threshold::regression_pct(10.0));
        let report = p.produce();
        assert_eq!(report.checks.len(), 1);
    }
}
