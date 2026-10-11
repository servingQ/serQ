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

/// Seeds name independent runs: seed 2's session k does not draw what seed
/// 1's session k+1 drew. A session's stream once mixed the seed and its
/// serial only by addition, so seeds 1..N were one sample path shifted by a
/// session each (#297).
#[test]
fn consecutive_seeds_do_not_replay_each_others_sessions() {
    let src = "fn main() {
  stage svc : fifo;
  workload { arrive batch(40); session { set x = ~exp(1); turn; observe drawn = x; end; } }
  server { run svc (cost(svc, 1)); }
}";
    let draws = |seed: u64| -> std::collections::BTreeMap<u64, f64> {
        let overrides = Overrides {
            seed: Some(seed),
            ..common::horizon(1e3)
        };
        let r = run_ir(&compile_source(src, &overrides).unwrap(), None).unwrap();
        let o = r.observe("drawn").unwrap();
        o.records
            .iter()
            .map(|r| r.1)
            .zip(o.samples.iter().copied())
            .collect()
    };
    let (one, two) = (draws(1), draws(2));
    assert_eq!(one.len(), 40);
    let replayed = two
        .iter()
        .filter(|&(k, x)| one.get(&(k + 1)) == Some(x))
        .count();
    assert_eq!(replayed, 0, "seed 2 replays seed 1 a session later");
}
