use dev_bench::{Benchmark, Threshold};
use std::time::Duration;

#[test]
fn smoke_basic_run() {
    let mut b = Benchmark::new("noop");
    for _ in 0..100 {
        b.iter(|| std::hint::black_box(1 + 1));
    }
    let r = b.finish();
    assert_eq!(r.samples.len(), 100);
    assert_eq!(r.name, "noop");
}

#[test]
fn smoke_no_baseline_skips() {
    let mut b = Benchmark::new("x");
    b.iter(|| ());
    let r = b.finish();
    let v = r.compare_against_baseline(None, Threshold::regression_pct(10.0));
    assert!(matches!(v.verdict, dev_report::Verdict::Skip));
}

#[test]
fn smoke_within_threshold_passes() {
    let mut b = Benchmark::new("x");
    for _ in 0..5 {
        b.iter(|| std::thread::sleep(Duration::from_micros(1)));
    }
    let r = b.finish();
    let v = r.compare_against_baseline(Some(r.mean), Threshold::regression_pct(20.0));
    assert!(matches!(v.verdict, dev_report::Verdict::Pass));
}
