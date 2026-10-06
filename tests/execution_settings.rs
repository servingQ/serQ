//! A model describes the deployment; an invocation supplies the experiment.
mod common;

use common::{Fixture, failure};
use serq::{Overrides, compile_source, run_ir};

const MODEL: &str = "fn main() {
  pool slots { cap 2; }
  stage svc : fifo;
  workload { arrive batch(4); session { set t0 = now; turn; observe latency = now - t0; end; } }
  server { hold slots (cost(slots, 1)) { run svc (cost(svc, 2)); } }
  gauge occupied = used(slots);
}";

#[test]
fn settings_are_required_outside_the_model_and_recorded_in_ir() {
    let err = compile_source(MODEL, &Overrides::default()).unwrap_err();
    assert!(err.contains("no horizon"), "{err}");
    let p = compile_source(MODEL, &common::horizon(10.0)).unwrap();
    let restored = serq::Program::from_json(&p.to_json()).unwrap();
    assert_eq!(
        (restored.horizon, restored.warmup, restored.seed),
        (10.0, 0.0, 1)
    );
    let r = run_ir(&restored, None).unwrap();
    // fifo has one server: each of four jobs takes two seconds. The pool
    // admits two sessions, but does not add a second server to the stage.
    assert_eq!(r.observe("latency").unwrap().samples, [2.0, 4.0, 6.0, 8.0]);
    assert_eq!(r.gauge("occupied").unwrap().mean, 1.4);
}

#[test]
fn model_settings_are_rejected_even_when_the_caller_supplies_them() {
    let source = MODEL.replacen("fn main() {", "fn main() { run { horizon 10; }", 1);
    for options in [Overrides::default(), common::horizon(20.0)] {
        let e = compile_source(&source, &options).unwrap_err();
        assert!(
            e.contains("execution settings do not belong in a model"),
            "{e}"
        );
        assert!(e.contains("--horizon") && e.contains("--instance"), "{e}");
    }
    assert!(serq::frontend::fmt::format(&source).is_err());
}

#[test]
fn cli_inspection_needs_no_experiment_but_run_and_ir_do() {
    let f = Fixture::new();
    f.write("model.sq", MODEL);
    // A nearby file is never an implicit source of settings.
    f.write("experiment.sq", "run { horizon 10; }");
    for cmd in ["check", "draw", "fmt"] {
        let out = f.run(&[cmd, "model.sq"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    for cmd in ["run", "ir"] {
        failure(&f.run(&[cmd, "model.sq"]), 1, &["no horizon"]);
        let out = f.run(&[cmd, "model.sq", "--horizon", "10"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out = f.run(&[cmd, "model.sq", "--instance", "experiment.sq"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let ir = f.run(&["ir", "model.sq", "--horizon", "10"]);
    f.write("model.json", std::str::from_utf8(&ir.stdout).unwrap());
    assert!(f.run(&["run", "model.json"]).status.success());
}
