//! `serve only (p)` (#261): an iteration serves the residents `p` reads as
//! nonzero, in the order `serve` names. FasterTransformer as Dai et al.
//! model it (arXiv 2504.07347 §4, decode first, no mixed batching) is the
//! case that asked for it. Unit step costs make the schedule explicit.

use serq::{Overrides, Program, compile_source, run_ir, run_source};
use std::path::Path;
use std::process::Command;

const FT: &str = "serve only (decoders > 0 ? decoding : !decoding);";

/// A prefills 3 and decodes 2 from t=0; B arrives at t=1 and does the same.
fn source(serve: &str) -> String {
    format!(
        r#"
        pool reqs {{ cap 4; }}
        stage gate : delay;
        stage engine : step {{ budget 8; cost 1; {serve} }}
        workload {{ arrive batch(2); }}
        session {{
          run gate (serial);
          hold reqs (1) {{
            run engine prefill (3);
            run engine decode (2);
          }}
          observe done = now;
          end;
        }}
        run {{ horizon 20; }}
        "#
    )
}

fn trace(name: &str, src: &str) -> Vec<String> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("serve-only");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.sq"));
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_serq"))
        .env("SERQ_TRACE_ITER", "1")
        .args(["run", path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(out.status.success(), "{stderr}");
    stderr
        .lines()
        .filter(|l| l.starts_with("ITER "))
        .map(|l| l.split(" | ").next().unwrap().to_owned())
        .collect()
}

#[test]
fn decode_first_mixes_and_fastertransformer_does_not() {
    // Sarathi: B's prefill joins A's decode at t=1.
    assert_eq!(
        trace("sarathi", &source("serve decode first;")),
        [
            "ITER 0.0000 0:0:p3",
            "ITER 1.0000 0:0:d1 1:0:p3",
            "ITER 2.0000 0:0:d1 1:0:d1",
            "ITER 3.0000 1:0:d1",
        ]
    );
    // FasterTransformer: B is resident from t=1 and waits until A has
    // decoded its last token; no iteration mixes the two.
    assert_eq!(
        trace("ft", &source(FT)),
        [
            "ITER 0.0000 0:0:p3",
            "ITER 1.0000 0:0:d1",
            "ITER 2.0000 0:0:d1",
            "ITER 3.0000 1:0:p3",
            "ITER 4.0000 1:0:d1",
            "ITER 5.0000 1:0:d1",
        ]
    );
}

#[test]
fn the_opposite_is_written_with_the_same_construct() {
    // Prefills alone while one is resident (design criterion 2): A's decode
    // waits for B's prefill.
    let src = source("serve only (decoders < residents ? !decoding : decoding);");
    assert_eq!(
        trace("prefill-alone", &src),
        [
            "ITER 0.0000 0:0:p3",
            "ITER 1.0000 1:0:p3",
            "ITER 2.0000 0:0:d1 1:0:d1",
            "ITER 3.0000 0:0:d1 1:0:d1",
        ]
    );
}

#[test]
fn only_selects_and_by_orders() {
    // Both arrive at 0, A with 4 prefill tokens, B with 2, budget 2. `by
    // (remaining)` serves B's prefill first; `only` then holds A's prefill
    // while B decodes.
    let src = r#"
        pool reqs { cap 4; }
        stage engine : step {
          budget 2; cost 1;
          serve only (decoders > 0 ? decoding : !decoding) by (remaining);
        }
        workload { arrive batch(2); init { set prompt = serial == 0 ? 4 : 2; } }
        session {
          hold reqs (1) { run engine prefill (prompt); run engine decode (1); }
          end;
        }
        run { horizon 20; }
    "#;
    assert_eq!(
        trace("by", src),
        [
            "ITER 0.0000 1:0:p2",
            "ITER 1.0000 1:0:d1",
            "ITER 2.0000 0:0:p2",
            "ITER 3.0000 0:0:p2",
            "ITER 4.0000 0:0:d1",
        ]
    );
}

/// A prefills 1 and decodes 2 from t=0; B and C wait from t=1 in a queue
/// the engine admits, and do the same.
fn admitted(serve: &str) -> String {
    format!(
        r#"
        pool reqs {{ cap 4; admit via engine; }}
        stage gate : delay;
        stage engine : step {{ budget 8; cost 1; {serve} }}
        workload {{ arrive batch(3); }}
        session {{
          run gate (serial > 0 ? 1 : 0);
          hold reqs (1) {{
            observe admitted = now;
            run engine prefill (1);
            run engine decode (2);
          }}
          end;
        }}
        run {{ horizon 20; }}
        "#
    )
}

#[test]
fn an_excluded_admission_waits_as_a_resident() {
    // B and C are admitted at t=1, where A decodes: both are excluded, keep
    // their slot of `reqs`, and prefill together at t=3, when A is done.
    // How many are admitted is the pool's `cap`, not `only`'s.
    let src = admitted(FT);
    assert_eq!(
        trace("admitted", &src),
        [
            "ITER 0.0000 0:0:p1",
            "ITER 1.0000 0:0:d1",
            "ITER 2.0000 0:0:d1",
            "ITER 3.0000 1:0:p1 2:0:p1",
            "ITER 4.0000 1:0:d1 2:0:d1",
            "ITER 5.0000 1:0:d1 2:0:d1",
        ]
    );
    let r = run_source(&src, &Overrides::default(), None).unwrap();
    assert_eq!(r.ended, 3);
    assert_eq!(r.observe("admitted").unwrap().samples, [0.0, 1.0, 1.0]);
}

#[test]
fn a_session_admitted_in_the_iteration_counts_among_the_residents() {
    // `residents` and `decoders` are read on the residents as they stand: a
    // session the iteration admits is one of them. All three arrive at an
    // empty engine. Read on the residents before the admission, the opposite
    // rule saw each as a prefill among none (0 < 0 is false), served it
    // nothing, and the engine never ran.
    let src = admitted("serve only (decoders < residents ? !decoding : decoding);")
        .replace("run gate (serial > 0 ? 1 : 0);", "");
    assert_eq!(
        trace("admitted-opposite", &src),
        [
            "ITER 0.0000 0:0:p1 1:0:p1 2:0:p1",
            "ITER 1.0000 0:0:d1 1:0:d1 2:0:d1",
            "ITER 2.0000 0:0:d1 1:0:d1 2:0:d1",
        ]
    );
    // A resident served earlier in the iteration is not reconsidered: at
    // t=1 A's decode is served before B and C are admitted, and they join
    // it. Displacing a served decode for a waiting prefill is
    // `exclusive prefill`'s admission rule, which `only` does not have.
    let src = admitted("serve only (decoders < residents ? !decoding : decoding);");
    assert_eq!(
        trace("admitted-joins", &src)[1],
        "ITER 1.0000 0:0:d1 1:0:p1 2:0:p1"
    );
}

#[test]
fn the_ir_runs_as_the_text_and_omits_an_absent_only() {
    let src = source(FT);
    let p = compile_source(&src, &Overrides::default()).unwrap();
    let json = p.to_json();
    assert!(json.contains("\"only\""));
    let from_ir = run_ir(&Program::from_json(&json).unwrap(), None).unwrap();
    let from_text = run_source(&src, &Overrides::default(), None).unwrap();
    assert_eq!(from_text.json(), from_ir.json());
    assert_eq!(from_text.observe("done").unwrap().samples, [3.0, 6.0]);
    let plain = compile_source(&source("serve decode first;"), &Overrides::default()).unwrap();
    assert!(!plain.to_json().contains("\"only\""));
}

#[test]
fn an_engine_that_excludes_every_resident_waits_for_the_residents_to_change() {
    // A alone is excluded from t=0: no iteration runs. B joins at t=2, and
    // the next instant serves both.
    let src = r#"
        pool reqs { cap 4; }
        stage gate : delay;
        stage engine : step { budget 8; cost 1; serve only (residents > 1); }
        workload { arrive batch(2); }
        session {
          run gate (2 * serial);
          hold reqs (1) { run engine prefill (1); run engine decode (1); }
          end;
        }
        run { horizon 20; }
    "#;
    assert_eq!(
        trace("waits", src),
        ["ITER 2.0000 0:0:p1 1:0:p1", "ITER 3.0000 0:0:d1 1:0:d1"]
    );
}

#[test]
fn only_is_refused_where_it_is_ambiguous_or_unreadable() {
    for (serve, message) in [
        ("serve only (decoding) exclusive prefill;", "a third rule"),
        ("serve only (~exp(1) > 1);", "may not draw"),
        ("serve only (tokens > 0);", "tokens"),
        ("serve only (now >= 5 || decoding);", "may not read `now`"),
    ] {
        let error = compile_source(&source(serve), &Overrides::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{serve}: {error}");
    }
    // the IR refuses what the parser does: `only` is a body, which the
    // exclusive rule cannot sit beside
    let mut p = compile_source(&source(FT), &Overrides::default()).unwrap();
    let serq::ir::CStageKind::Step(st) = &mut p.stages[1].kind else {
        panic!("engine is a step stage")
    };
    st.serve = serq::ir::CServe::ExclusivePrefill;
    assert!(
        Program::from_json(&p.to_json())
            .unwrap_err()
            .contains("takes back")
    );
}
