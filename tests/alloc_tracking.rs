//! End-to-end check of the `alloc-tracking` feature: install the
//! allocator through the crate's macro and measure a known allocation.
#![cfg(feature = "alloc-tracking")]

use dev_bench::alloc::AllocationStats;
use dev_report::Verdict;

dev_bench::install_global_allocator!();

#[test]
fn macro_installs_tracking_allocator_and_since_measures_a_region() {
    let before = AllocationStats::snapshot();
    let v: Vec<u8> = std::hint::black_box(vec![7u8; 1 << 20]);
    let after = AllocationStats::snapshot();
    drop(v);

    let region = after.since(&before);
    // Other test threads may allocate concurrently, so only lower bounds
    // are exact.
    assert!(region.total_bytes >= 1 << 20, "{region:?}");
    assert!(region.total_blocks >= 1, "{region:?}");
    assert!(after.peak_bytes >= 1 << 20, "{after:?}");

    // The raw snapshot is cumulative since program start.
    assert!(after.total_bytes >= region.total_bytes);

    let check = region.compare_against_baseline("vec_1mib", Some(region), 10.0);
    assert_eq!(check.verdict, Verdict::Pass);
}
