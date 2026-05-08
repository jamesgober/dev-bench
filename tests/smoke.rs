use dev_bench::{
    Baseline, BaselineStore, BenchProducer, Benchmark, JsonFileBaselineStore, Threshold,
};
use dev_report::Producer;
use std::time::Duration;

#[test]
fn smoke_basic_run() {
    let mut b = Benchmark::new("noop");
    for _ in 0..100 {
        b.iter(|| std::hint::black_box(1 + 1));
    }
    let r = b.finish();
    assert_eq!(r.samples.len(), 100);
    assert_eq!(r.iterations_recorded, 100);
    assert_eq!(r.name, "noop");
}

#[test]
fn smoke_no_baseline_skips() {
    let mut b = Benchmark::new("x");
    b.iter(|| ());
    let r = b.finish();
    let v = r.compare_against_baseline(None, Threshold::regression_pct(10.0));
    assert!(matches!(v.verdict, dev_report::Verdict::Skip));
    assert!(v.has_tag("bench"));
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

#[test]
fn smoke_check_carries_numeric_evidence() {
    let mut b = Benchmark::new("x");
    for _ in 0..5 {
        b.iter(|| std::thread::sleep(Duration::from_micros(1)));
    }
    let r = b.finish();
    let v = r.compare_against_baseline(Some(r.mean), Threshold::regression_pct(20.0));

    // bench tag and numeric evidence are attached.
    assert!(v.has_tag("bench"));
    let labels: Vec<&str> = v.evidence.iter().map(|e| e.label.as_str()).collect();
    assert!(labels.contains(&"mean_ns"));
    assert!(labels.contains(&"baseline_ns"));
    assert!(labels.contains(&"p50_ns"));
    assert!(labels.contains(&"p99_ns"));
    assert!(labels.contains(&"cv"));
    assert!(labels.contains(&"ops_per_sec"));
    assert!(labels.contains(&"samples"));
    assert!(labels.contains(&"iterations_recorded"));
}

#[test]
fn smoke_baseline_store_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonFileBaselineStore::new(dir.path());
    let b = Baseline {
        name: "parse".into(),
        mean_ns: 1500,
        samples: 100,
        ops_per_sec: 666_666.0,
    };
    store.save("main", &b).unwrap();
    let back = store.load("main", "parse").unwrap().unwrap();
    assert_eq!(back, b);

    // Now use the loaded baseline in a comparison.
    let mut bench = Benchmark::new("parse");
    for _ in 0..5 {
        bench.iter(|| std::hint::black_box(1 + 1));
    }
    let r = bench.finish();
    let v = r.compare_against_baseline(Some(back.mean()), Threshold::regression_pct(50.0));
    assert!(v.has_tag("bench"));
}

#[test]
fn smoke_producer_emits_finalized_report() {
    fn run() -> dev_bench::BenchmarkResult {
        let mut b = Benchmark::new("hot_path");
        for _ in 0..5 {
            b.iter(|| std::hint::black_box(1 + 1));
        }
        b.finish()
    }
    let producer = BenchProducer::new(run, "0.1.0", None, Threshold::regression_pct(10.0));
    let report = producer.produce();
    assert_eq!(report.checks.len(), 1);
    assert_eq!(report.producer.as_deref(), Some("dev-bench"));
    // Subject is the benchmark name.
    assert_eq!(report.subject, "hot_path");
    // No baseline -> Skip.
    assert!(matches!(
        report.overall_verdict(),
        dev_report::Verdict::Skip
    ));
}
