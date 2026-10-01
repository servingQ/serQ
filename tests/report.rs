//! The shape of `Report::json`, which consumers read by field name: a
//! renamed, removed or retyped field is a change of `REPORT_VERSION`, an
//! added one is recorded here (`docs/ir.md`, Stability).

use serq::engine::report::REPORT_VERSION;
use serq::{Overrides, compile_source, run_ir};

fn keys(v: &serde_json::Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

#[test]
fn the_report_has_the_shape_its_version_names() {
    let src = "pool kv { cap 10; } stage svc : fifo;
        workload { arrive poisson(1); }
        session { hold kv (1) { run svc (~exp(0.5)); } observe x = now; end; }
        gauge g = used(kv);
        run { horizon 100; }";
    let p = compile_source(src, &Overrides::default()).unwrap();
    let j: serde_json::Value = serde_json::from_str(&run_ir(&p, None).unwrap().json()).unwrap();
    let shape = (
        keys(&j),
        keys(&j["observes"]["x"]),
        keys(&j["gauges"]["g"]),
        keys(&j["stages"][0]),
        keys(&j["pools"][0]),
    );
    let want = (
        vec![
            "arrivals",
            "end",
            "ended",
            "events",
            "gauges",
            "horizon",
            "mean_live",
            "observes",
            "pools",
            "seed",
            "stages",
            "turns",
            "warmup",
        ],
        vec!["ci", "count", "cv2", "mean", "p99"],
        vec!["ci", "max", "mean", "min"],
        vec![
            "completed",
            "iterations",
            "mean_number",
            "mean_service",
            "mean_wait",
            "name",
            "throughput",
            "utilization",
        ],
        vec![
            "admissions",
            "evicted_entries",
            "evicted_units",
            "mean_cached",
            "mean_holders",
            "mean_queue",
            "mean_used",
            "mean_wait",
            "name",
            "preemptions",
            "rejected",
            "spills",
            "stuck",
        ],
    );
    let want = (
        want.0.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        want.1.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        want.2.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        want.3.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        want.4.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    );
    assert_eq!(
        shape, want,
        "the report's shape moved: a renamed, removed or retyped field bumps REPORT_VERSION ({REPORT_VERSION}); an added one is recorded here; say so in the release"
    );
}
