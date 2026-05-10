//! Allocation tracking. Available with the `alloc-tracking` feature.
//!
//! Wraps `dhat` to capture total bytes, total allocation count, and
//! peak resident bytes during a benchmark. Reports a [`CheckResult`]
//! with regression-style verdict.
//!
//! ## Cost
//!
//! Enabling `alloc-tracking` installs `dhat::Alloc` as the global
//! allocator. This is heavier than the default allocator and
//! materially changes timing characteristics. **Do not combine
//! allocation thresholds with timing thresholds in the same
//! invocation.** Run timing benchmarks with the feature off and
//! allocation benchmarks with it on.
//!
//! ## Setup
//!
//! Use the [`install_global_allocator!`](crate::install_global_allocator)
//! macro at module scope in your binary or test target:
//!
//! ```ignore
//! dev_bench::install_global_allocator!();
//! ```
//!
//! Then start a profiler before the benchmark and snapshot stats after:
//!
//! ```ignore
//! let _profiler = dhat::Profiler::new_heap();
//! // ... run benchmarked code ...
//! let stats = dev_bench::alloc::AllocationStats::snapshot();
//! ```
//!
//! The macro expands to:
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOC: dhat::Alloc = dhat::Alloc;
//! ```

use dev_report::{CheckResult, Evidence, Severity};

/// Snapshot of allocation activity, captured from `dhat::HeapStats`.
///
/// Build via [`AllocationStats::snapshot`] inside a `dhat::Profiler`
/// scope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AllocationStats {
    /// Total bytes allocated across the profiled scope (cumulative).
    pub total_bytes: u64,
    /// Total number of allocations across the profiled scope.
    pub total_blocks: u64,
    /// Peak bytes resident at any one time.
    pub peak_bytes: u64,
    /// Peak number of blocks resident at any one time.
    pub peak_blocks: u64,
}

impl AllocationStats {
    /// Capture the current `dhat::HeapStats` into an `AllocationStats`.
    ///
    /// MUST be called inside an active `dhat::Profiler::new_heap()`
    /// scope. Outside that scope, `dhat` panics.
    pub fn snapshot() -> Self {
        let s = dhat::HeapStats::get();
        Self {
            total_bytes: s.total_bytes,
            total_blocks: s.total_blocks,
            peak_bytes: s.max_bytes as u64,
            peak_blocks: s.max_blocks as u64,
        }
    }

    /// Compare this snapshot against a baseline.
    ///
    /// `pct_threshold` is the maximum tolerated growth in
    /// `total_bytes` over the baseline. A regression beyond the
    /// threshold yields `Fail (Warning)`. No baseline yields `Skip`.
    pub fn compare_against_baseline(
        &self,
        name: &str,
        baseline: Option<AllocationStats>,
        pct_threshold: f64,
    ) -> CheckResult {
        let check_name = format!("alloc::{}", name);
        let mut evidence = vec![
            Evidence::numeric("total_bytes", self.total_bytes as f64),
            Evidence::numeric("total_blocks", self.total_blocks as f64),
            Evidence::numeric("peak_bytes", self.peak_bytes as f64),
            Evidence::numeric("peak_blocks", self.peak_blocks as f64),
        ];

        let Some(base) = baseline else {
            let mut c = CheckResult::skip(check_name).with_detail("no allocation baseline");
            c.tags = vec!["alloc".to_string()];
            c.evidence = evidence;
            return c;
        };

        evidence.push(Evidence::numeric(
            "baseline_total_bytes",
            base.total_bytes as f64,
        ));
        let allowed = base.total_bytes as f64 * (1.0 + pct_threshold / 100.0);
        let regressed = (self.total_bytes as f64) > allowed;
        let detail = format!(
            "current_total_bytes={} baseline_total_bytes={} threshold_pct={}",
            self.total_bytes, base.total_bytes, pct_threshold
        );
        if regressed {
            let mut c = CheckResult::fail(check_name, Severity::Warning).with_detail(detail);
            c.tags = vec!["alloc".to_string(), "regression".to_string()];
            c.evidence = evidence;
            c
        } else {
            let mut c = CheckResult::pass(check_name).with_detail(detail);
            c.tags = vec!["alloc".to_string()];
            c.evidence = evidence;
            c
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dev_report::Verdict;

    fn synthetic(total_bytes: u64) -> AllocationStats {
        AllocationStats {
            total_bytes,
            total_blocks: 10,
            peak_bytes: total_bytes,
            peak_blocks: 5,
        }
    }

    #[test]
    fn no_baseline_skips() {
        let s = synthetic(1_000);
        let c = s.compare_against_baseline("x", None, 10.0);
        assert_eq!(c.verdict, Verdict::Skip);
        assert!(c.has_tag("alloc"));
    }

    #[test]
    fn within_threshold_passes() {
        let curr = synthetic(105);
        let base = synthetic(100);
        let c = curr.compare_against_baseline("x", Some(base), 10.0);
        assert_eq!(c.verdict, Verdict::Pass);
    }

    #[test]
    fn over_threshold_fails() {
        let curr = synthetic(120);
        let base = synthetic(100);
        let c = curr.compare_against_baseline("x", Some(base), 10.0);
        assert_eq!(c.verdict, Verdict::Fail);
        assert!(c.has_tag("regression"));
    }

    #[test]
    fn evidence_includes_all_metrics() {
        let curr = synthetic(100);
        let c = curr.compare_against_baseline("x", None, 10.0);
        let labels: Vec<&str> = c.evidence.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"total_bytes"));
        assert!(labels.contains(&"total_blocks"));
        assert!(labels.contains(&"peak_bytes"));
        assert!(labels.contains(&"peak_blocks"));
    }
}
