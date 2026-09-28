//! The IR is the definition of a program: every example program compiles to
//! IR that survives a JSON round trip unchanged, runs to the same report
//! from IR as from text, and malformed IR is rejected on load.

use std::path::Path;

use seq::{Overrides, Program, compile_source, run_ir, run_source};

fn programs() -> Vec<std::path::PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("programs");
    let mut v: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "seq"))
        .collect();
    v.sort();
    assert!(v.len() >= 10, "example programs");
    v
}

/// A short run of every program, so the comparison is cheap.
fn short() -> Overrides {
    Overrides {
        horizon: Some(60.0),
        warmup: Some(0.0),
        seed: Some(3),
        ..Default::default()
    }
}

#[test]
fn json_round_trip_is_exact() {
    for path in programs() {
        let src = std::fs::read_to_string(&path).unwrap();
        let p = compile_source(&src, &Overrides::default()).unwrap();
        let j = p.to_json();
        let q = Program::from_json(&j).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(j, q.to_json(), "{}", path.display());
    }
}

#[test]
fn ir_runs_like_text() {
    for path in programs() {
        let src = std::fs::read_to_string(&path).unwrap();
        let base = path.parent();
        let from_text = run_source(&src, &short(), base).unwrap().text();
        let p = compile_source(&src, &short()).unwrap();
        let q = Program::from_json(&p.to_json()).unwrap();
        let from_ir = run_ir(&q, base).unwrap().text();
        assert_eq!(from_text, from_ir, "{}", path.display());
    }
}

#[test]
fn malformed_ir_is_rejected() {
    let src = std::fs::read_to_string(seq::program_path("mg1")).unwrap();
    let p = compile_source(&src, &Overrides::default()).unwrap();
    let mut bad = p.clone();
    bad.version = 0;
    assert!(bad.validate().unwrap_err().contains("version"));
    let mut bad = p.clone();
    bad.session = bad.blocks.len();
    assert!(bad.validate().unwrap_err().contains("out of range"));
    let mut bad = p.clone();
    bad.blocks[bad.session].push(seq::ir::CStmt::Set(99, seq::ir::CExpr::Num(1.0)));
    assert!(bad.validate().unwrap_err().contains("attribute slot 99"));
    let j = p.to_json().replace("\"Fifo\": 1", "\"Fifo\": \"x\"");
    assert!(Program::from_json(&j).is_err());
}

#[test]
fn explicit_sessions_preset_attributes() {
    // two sessions with different service times through a delay stage
    let src = r#"
        stage d : delay;
        workload { arrive batch(1); init { set w = 1; } }
        session { run d (w); observe done = now; end; }
        run { horizon 100; }
    "#;
    let p = compile_source(src, &Overrides::default())
        .unwrap()
        .with_sessions(&[vec![("w", 5.0)], vec![("w", 2.0)]])
        .unwrap();
    let r = run_ir(&p, None).unwrap();
    let done = &r.observe("done").unwrap().records;
    let by_serial: Vec<(u64, f64)> = {
        let mut v: Vec<_> = done.iter().map(|&(t, s, _)| (s, t)).collect();
        v.sort_by_key(|a| a.0);
        v
    };
    assert_eq!(by_serial, vec![(0, 5.0), (1, 2.0)]);
    assert!(
        compile_source(src, &Overrides::default())
            .unwrap()
            .with_sessions(&[vec![("nope", 1.0)]])
            .is_err()
    );
}

#[test]
fn inlined_trace_runs_like_the_corpus() {
    // the calibrated A100 replay over the full short-context trace (333
    // sessions): the trace file and its sessions inlined into the IR give
    // the same run
    let path = seq::program_path("vllm_replay");
    let src = std::fs::read_to_string(&path).unwrap();
    let ov = Overrides {
        horizon: Some(1500.0),
        ..Default::default()
    };
    let p = compile_source(&src, &ov).unwrap();
    let base = path.parent();
    let from_trace = run_ir(&p, base).unwrap().text();
    let inlined = seq::inline_trace(p, base).unwrap();
    assert!(inlined.trace.is_none());
    let q = Program::from_json(&inlined.to_json()).unwrap();
    let from_ir = run_ir(&q, None).unwrap().text();
    assert_eq!(from_trace, from_ir);
}
