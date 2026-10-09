//! Allocation tracking. Available with the `alloc-tracking` feature.
//!
//! Wraps `mod-alloc`'s `dhat_compat` surface (a drop-in replacement
//! for `dhat-rs`) to capture total bytes, total allocation count,
//! and peak resident bytes during a benchmark. Reports a
//! [`CheckResult`] with regression-style verdict.
//!
//! As of v0.9.7 the backend is `mod-alloc` instead of `dhat`; the
//! public API surface here (`AllocationStats`, the
//! `install_global_allocator!` macro) is unchanged.
//!
//! ## Cost
//!
//! Enabling `alloc-tracking` installs a tracking global allocator.
//! It is heavier than the default allocator and changes timing
//! characteristics. With `mod-alloc` 1.0.1 or later it only updates
//! counters: call sites are recorded only while a
//! `mod_alloc::dhat_compat::Profiler` is alive, and dev-bench never
//! creates one, so no time goes into walking stack frames. **Do not combine allocation thresholds with
//! timing thresholds in the same invocation.** Run timing
//! benchmarks with the feature off and allocation benchmarks with
//! it on.
//!
//! ## Setup
//!
//! Use the [`install_global_allocator!`](crate::install_global_allocator)
//! macro at module scope in your binary or test target. It names the
//! allocator through `dev-bench`, so your crate does not need its own
//! `mod-alloc` dependency:
//!
//! ```ignore
//! dev_bench::install_global_allocator!();
//! ```
//!
//! The counters behind [`AllocationStats::snapshot`] are process-wide
//! and run from program start. To measure one region, take a snapshot
//! before and after and use [`AllocationStats::since`]:
//!
//! ```ignore
//! use dev_bench::alloc::AllocationStats;
//!
//! let before = AllocationStats::snapshot();
//! // ... run benchmarked code ...
//! let stats = AllocationStats::snapshot().since(&before);
//! let check = stats.compare_against_baseline("parse", baseline_alloc, 10.0);
//! ```
//!
//! No `Profiler` is needed for the counters. `mod_alloc::dhat_compat::Profiler`
//! turns on call-site recording while it is alive and writes a DHAT JSON
//! file when dropped; it does not reset or scope the counters. If you want that file, add `mod-alloc` with the
//! `dhat-compat` feature at the same major version `dev-bench` uses
//! (currently `1`), otherwise your `Profiler` belongs to a different copy
//! of `mod-alloc` than the installed allocator and records nothing.
//!
//! The macro expands to a `#[global_allocator] static` of
//! `mod_alloc::dhat_compat::Alloc`.

use dev_report::{CheckResult, Evidence, Severity};

/// Snapshot of allocation activity, captured from
/// `mod_alloc::dhat_compat::HeapStats` (drop-in for
/// `dhat::HeapStats`).
///
/// Build via [`AllocationStats::snapshot`]. A raw snapshot holds
/// process-wide totals since program start; use
/// [`AllocationStats::since`] to get the activity of one region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AllocationStats {
    /// Total bytes allocated (cumulative). Since program start for a
    /// raw [`snapshot`](AllocationStats::snapshot), or within the region
    /// for a value returned by [`since`](AllocationStats::since).
    pub total_bytes: u64,
    /// Total number of allocations, with the same scope as `total_bytes`.
    pub total_blocks: u64,
    /// Peak bytes resident at any one time since program start.
    pub peak_bytes: u64,
    /// Peak number of blocks resident at any one time since program start.
    pub peak_blocks: u64,
}

impl AllocationStats {
    /// Capture the installed allocator's counters.
    ///
    /// The counters are process-wide and cumulative from program start;
    /// a `mod_alloc::dhat_compat::Profiler` does not reset them. Returns
    /// zeros if the tracking allocator is not installed (see
    /// [`install_global_allocator!`](crate::install_global_allocator)).
    /// Unlike `dhat-rs`, this never panics outside a `Profiler` scope.
    pub fn snapshot() -> Self {
        let s = mod_alloc::dhat_compat::HeapStats::get();
        Self {
            total_bytes: s.total_bytes,
            total_blocks: s.total_blocks,
            peak_bytes: s.max_bytes as u64,
            peak_blocks: s.max_blocks as u64,
        }
    }

    /// Allocation activity between an `earlier` snapshot and this one.
    ///
    /// `total_bytes` and `total_blocks` become the difference (saturating
    /// at zero). `peak_bytes` and `peak_blocks` keep this snapshot's
    /// values: the allocator only tracks a lifetime high-water mark, so a
    /// per-region peak cannot be derived from two snapshots.
    ///
    /// # Example
    ///
    /// ```
    /// use dev_bench::alloc::AllocationStats;
    ///
    /// let before = AllocationStats {
    ///     total_bytes: 1_000,
    ///     total_blocks: 10,
    ///     peak_bytes: 600,
    ///     peak_blocks: 4,
    /// };
    /// let after = AllocationStats {
    ///     total_bytes: 1_512,
    ///     total_blocks: 13,
    ///     peak_bytes: 800,
    ///     peak_blocks: 5,
    /// };
    /// let region = after.since(&before);
    /// assert_eq!(region.total_bytes, 512);
    /// assert_eq!(region.total_blocks, 3);
    /// assert_eq!(region.peak_bytes, 800);
    /// ```
    pub fn since(&self, earlier: &AllocationStats) -> AllocationStats {
        AllocationStats {
            total_bytes: self.total_bytes.saturating_sub(earlier.total_bytes),
            total_blocks: self.total_blocks.saturating_sub(earlier.total_blocks),
            peak_bytes: self.peak_bytes,
            peak_blocks: self.peak_blocks,
        }
    }

    /// Compare this snapshot against a baseline.
    ///
    /// `pct_threshold` is the maximum tolerated growth in
    /// `total_bytes` over the baseline. A regression beyond the
    /// threshold yields `Fail (Warning)`. No baseline yields `Skip`.
    /// A non-finite `pct_threshold` (NaN or infinite) also yields `Skip`
    /// with a detail, because it would otherwise pass every run.
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
        if !pct_threshold.is_finite() {
            let mut c = CheckResult::skip(check_name).with_detail(format!(
                "threshold percent is not a finite number ({pct_threshold})"
            ));
            c.tags = vec!["alloc".to_string()];
            c.evidence = evidence;
            return c;
        }
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
    fn non_finite_threshold_skips() {
        let curr = synthetic(1_000_000);
        let base = synthetic(100);
        for pct in [f64::NAN, f64::INFINITY] {
            let c = curr.compare_against_baseline("x", Some(base), pct);
            assert_eq!(c.verdict, Verdict::Skip, "pct={pct}");
            assert!(c.detail.as_deref().unwrap().contains("not a finite number"));
        }
    }

    #[test]
    fn since_subtracts_totals_and_keeps_peaks() {
        let before = synthetic(100);
        let after = AllocationStats {
            total_bytes: 350,
            total_blocks: 14,
            peak_bytes: 900,
            peak_blocks: 7,
        };
        let d = after.since(&before);
        assert_eq!(d.total_bytes, 250);
        assert_eq!(d.total_blocks, 4);
        assert_eq!(d.peak_bytes, 900);
        assert_eq!(d.peak_blocks, 7);
        // Arguments in the wrong order saturate instead of wrapping.
        let w = before.since(&after);
        assert_eq!(w.total_bytes, 0);
        assert_eq!(w.total_blocks, 0);
    }

    #[test]
    fn snapshot_without_installed_allocator_is_zero_or_monotonic() {
        // The lib test binary does not install the tracking allocator,
        // so the counters stay at zero. Must not panic.
        let a = AllocationStats::snapshot();
        let _v: Vec<u8> = vec![0; 4096];
        let b = AllocationStats::snapshot();
        assert!(b.total_bytes >= a.total_bytes);
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
