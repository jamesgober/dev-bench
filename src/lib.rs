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
//!         // code under measurement
//!         std::hint::black_box(40 + 2);
//!     });
//! }
//!
//! let result = b.finish();
//! let threshold = Threshold::regression_pct(10.0);   // fail on +10%
//! let verdict = result.compare_against_baseline(None, threshold);
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
#![warn(rust_2018_idioms)]

use std::time::{Duration, Instant};

use dev_report::{CheckResult, Severity};

/// A single benchmark run.
pub struct Benchmark {
    name: String,
    samples: Vec<Duration>,
}

impl Benchmark {
    /// Begin a new benchmark with a stable name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            samples: Vec::new(),
        }
    }

    /// Run one iteration of the benchmark, capturing the duration.
    pub fn iter<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        let start = Instant::now();
        let r = f();
        let elapsed = start.elapsed();
        self.samples.push(elapsed);
        r
    }

    /// Finalize the benchmark and produce a result summary.
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
        BenchmarkResult {
            name: self.name,
            samples: self.samples,
            mean,
            p50,
            p99,
        }
    }
}

/// The result of a finished benchmark.
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    /// Stable name of the benchmark.
    pub name: String,
    /// All raw sample durations.
    pub samples: Vec<Duration>,
    /// Mean sample duration.
    pub mean: Duration,
    /// 50th percentile sample duration.
    pub p50: Duration,
    /// 99th percentile sample duration.
    pub p99: Duration,
}

/// A threshold defining how much slower-than-baseline is acceptable.
#[derive(Debug, Clone, Copy)]
pub enum Threshold {
    /// Fail if the new measurement is more than `pct` percent slower
    /// than the baseline.
    RegressionPct(f64),
    /// Fail if the new measurement is more than `nanos` slower than
    /// the baseline.
    RegressionAbsoluteNs(u128),
}

impl Threshold {
    /// Build a percent-based regression threshold.
    pub fn regression_pct(pct: f64) -> Self {
        Threshold::RegressionPct(pct)
    }

    /// Build an absolute regression threshold in nanoseconds.
    pub fn regression_abs_ns(nanos: u128) -> Self {
        Threshold::RegressionAbsoluteNs(nanos)
    }
}

impl BenchmarkResult {
    /// Compare this result against a baseline mean. If `baseline_mean`
    /// is `None`, the comparison is skipped and verdict is `Skip`.
    pub fn compare_against_baseline(
        &self,
        baseline_mean: Option<Duration>,
        threshold: Threshold,
    ) -> CheckResult {
        let Some(baseline) = baseline_mean else {
            return CheckResult::skip(format!("bench::{}", self.name))
                .with_detail("no baseline available");
        };
        let current_ns = self.mean.as_nanos();
        let baseline_ns = baseline.as_nanos();
        let regressed = match threshold {
            Threshold::RegressionPct(pct) => {
                let allowed = baseline_ns as f64 * (1.0 + pct / 100.0);
                current_ns as f64 > allowed
            }
            Threshold::RegressionAbsoluteNs(abs) => {
                current_ns.saturating_sub(baseline_ns) > abs
            }
        };
        let detail = format!(
            "current mean {} ns, baseline {} ns",
            current_ns, baseline_ns
        );
        let name = format!("bench::{}", self.name);
        if regressed {
            CheckResult::fail(name, Severity::Warning).with_detail(detail)
        } else {
            CheckResult::pass(name).with_detail(detail)
        }
    }
}

/// A trait for any object that can run a benchmark and produce a result.
pub trait Bench {
    /// Run the benchmark and return its result.
    fn run(&mut self) -> BenchmarkResult;
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
        assert!(r.mean > Duration::ZERO);
    }

    #[test]
    fn comparison_without_baseline_is_skip() {
        let mut b = Benchmark::new("x");
        b.iter(|| ());
        let r = b.finish();
        let v = r.compare_against_baseline(None, Threshold::regression_pct(5.0));
        assert_eq!(v.verdict, Verdict::Skip);
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
    }
}
