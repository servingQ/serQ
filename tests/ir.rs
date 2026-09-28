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

/// The serving vocabulary is sugar: a program written with `admit`,
/// `prefill`, `transfer`, `decode` and `tool` compiles to the IR of the
/// same program written with `hold` and `run`.
#[test]
fn serving_forms_compile_to_the_kernel_ir() {
    let deployment = r#"
        pool memP { cap 1000; evict lru; }
        pool memD { cap 1000; }
        stage prefill : fifo;
        stage link : ps(1);
        stage decode : ps(min(n, 4));
        stage tool : delay;
        workload {
          arrive poisson(0.5);
          init { set K = 0; }
          turn { set n = ~exp(100); set o = ~exp(20); set Z = ~exp(3); set T = K + n; }
        }
        run { horizon 100; seed 1; }
    "#;
    let serving = format!(
        "{deployment} session {{
            turn;
            loop {{
              admit memP (T) {{ prefill (n + K); transfer (T / 100); }} keep (T);
              admit memD (T) {{ decode (o); }}
              set K = T;
              branch with (0.8) {{ tool Z; turn; }} else {{ end; }}
            }}
        }}"
    );
    let kernel = format!(
        "{deployment} session {{
            turn;
            loop {{
              hold memP (T) {{ run prefill (n + K); run link (T / 100); }} cache (T);
              hold memD (T) {{ run decode (o); }}
              set K = T;
              branch with (0.8) {{ run tool (Z); turn; }} else {{ end; }}
            }}
        }}"
    );
    let a = compile_source(&serving, &Overrides::default()).unwrap();
    let b = compile_source(&kernel, &Overrides::default()).unwrap();
    assert_eq!(a.to_json(), b.to_json());
}

/// `branch with (p)` is a draw and says so. It rewrites at parse time to
/// `branch (~bernoulli(p))`, which the interpreter already treats identically
/// to a bare `branch (p)` — the sample and the guard draw from the same
/// stream with the same comparison, and the resulting 0/1 short-circuits the
/// guard without a second draw. So the sugar must be free: same IR as the
/// explicit form, and a run that does not move.
#[test]
fn branch_with_is_sugar_for_bernoulli() {
    let head = "stage tool : delay;
        workload { arrive poisson(0.5); turn { set Z = ~exp(3); } }
        run { horizon 2000; warmup 200; seed 1; }";
    let sugar = format!(
        "{head} session {{ turn; loop {{ branch with (0.8) {{ run tool (Z); turn; }} else {{ end; }} }} }}"
    );
    let explicit = format!(
        "{head} session {{ turn; loop {{ branch (~bernoulli(0.8)) {{ run tool (Z); turn; }} else {{ end; }} }} }}"
    );
    let ov = seq::Overrides::default();
    let a = seq::compile_source(&sugar, &ov).expect("the sugar compiles");
    let b = seq::compile_source(&explicit, &ov).expect("the explicit form compiles");
    assert_eq!(
        a.to_json(),
        b.to_json(),
        "`branch with` must reach the kernel as a bernoulli sample"
    );

    // and the bare guard it replaces is the same run, which is what lets the
    // corpus be rewritten without moving a single reported number
    let bare = format!(
        "{head} session {{ turn; loop {{ branch (0.8) {{ run tool (Z); turn; }} else {{ end; }} }} }}"
    );
    let one = seq::run_source(&sugar, &ov, None).expect("runs");
    let two = seq::run_source(&bare, &ov, None).expect("runs");
    assert_eq!(one.text(), two.text(), "the rewrite must not move a run");
}

/// A figure has to say which edges are draws. The lecture's own
/// `fig:deployment` writes "resume w.p. p" by hand.
#[test]
fn a_draw_is_labelled_w_p() {
    // a station before the branch, as every program in the corpus has: the
    // guard labels the edge that leaves it
    let src = "stage svc : fifo; stage tool : delay;
        workload { arrive poisson(0.5); turn { set Z = ~exp(3); } }
        session { turn; loop { run svc (1); branch with (0.8) { run tool (Z); turn; } else { end; } } }
        run { horizon 100; }";
    let p = seq::compile_source(src, &seq::Overrides::default()).unwrap();
    let svg = seq::svg::render(&seq::deployment::figure(&p));
    assert!(
        svg.contains("w.p. 0.8"),
        "the deployment view labels the draw"
    );
    assert!(
        !svg.contains("~bernoulli"),
        "and does not leak the desugaring"
    );
    let svg = seq::svg::render(&seq::draw::figure(&p, false));
    assert!(svg.contains("w.p. 0.8"), "the session view labels it too");
}
