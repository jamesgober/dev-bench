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
/// `dhat_compat::Alloc` / `Profiler` / `HeapStats` shapes mirror
/// `dhat-rs`'s public surface field-for-field; users following
/// dhat-rs's documentation pattern in their own code continue to
/// work via `use mod_alloc::dhat_compat as dhat;`.
///
/// Hidden from rustdoc; consumers should use the macro, not this path.
#[cfg(feature = "alloc-tracking")]
#[doc(hidden)]
pub use ::mod_alloc::dhat_compat as __dhat;

/// Install the allocation-tracking global allocator.
///
/// Available with the `alloc-tracking` feature. Invoke at module
/// scope in your binary or test target — the macro expands to a
/// `#[global_allocator] static` declaration that consumers cannot
/// otherwise express without depending on `mod-alloc` directly.
///
/// The backend is `mod-alloc`'s `dhat_compat::Alloc` (drop-in for
/// `dhat-rs`'s `dhat::Alloc`); behaviour, API surface, and JSON
/// output remain DHAT-viewer-compatible.
///
/// # Example
///
/// ```ignore
/// // in main.rs or a test target's top level:
/// dev_bench::install_global_allocator!();
///
/// // Optionally pull the same compat surface into your own code:
/// use mod_alloc::dhat_compat as dhat;
///
/// fn main() {
///     let _profiler = dhat::Profiler::new_heap();
///     // ... benchmarked code ...
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
        let r = f();
        let elapsed = start.elapsed();
        self.samples.push(elapsed);
        self.iterations_recorded += 1;
        r
    }

    /// Run a closure `n` times and record ONE sample for the entire batch.
    ///
    /// Use for sub-microsecond operations where per-iteration timing
    /// would be dominated by `Instant::now()` overhead. The reported
    /// per-iteration mean is `batch_duration / n`.
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
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
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
        let mean = if n == 0 {
            Duration::ZERO
        } else {
            let total: Duration = self.samples.iter().copied().sum();
            total / n as u32
        };
        let mut sorted = self.samples.clone();
        sorted.sort();
        let p50 = sorted.get(n / 2).copied().unwrap_or(Duration::ZERO);
        let p99 = sorted
            .get((n as f64 * 0.99).floor() as usize)
            .copied()
            .unwrap_or(Duration::ZERO);
        let cv = compute_cv(&self.samples, mean);
        let total_elapsed: Duration = self.samples.iter().copied().sum();
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
    /// 50th percentile sample duration.
    pub p50: Duration,
    /// 99th percentile sample duration.
    pub p99: Duration,
    /// Coefficient of variation across samples (stddev / mean).
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
    /// `MAD = median(|x_i - median(x)|)`. Less affected by outliers than
    /// standard deviation; useful for noisy measurements.
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
        deviations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = deviations.len() / 2;
        deviations[mid]
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
    pub fn percentile(&self, q: f64) -> Duration {
        if self.samples.is_empty() {
            return Duration::ZERO;
        }
        let q = q.clamp(0.0, 1.0);
        let mut sorted = self.samples.clone();
        sorted.sort();
        let n = sorted.len();
        let idx = ((n as f64) * q).floor() as usize;
        let idx = idx.min(n - 1);
        sorted[idx]
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
