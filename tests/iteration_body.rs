//! A step stage's iteration as the program writes it (`iteration { … }`,
//! #355): `serve`, `admit` and `branch`, run once each where written.

use serq::{Overrides, compile_source, run_source};

fn run(src: &str) -> serq::Report {
    run_source(src, &Overrides::default(), None).unwrap()
}

/// vLLM's procedure, written out.
const VLLM: &str = "iteration { serve; admit while (!preempted); }";

/// A stage without a body runs vLLM's procedure: every program of the
/// corpus whose engines have no `exclusive prefill` and no `serve only`
/// gives the same report with the procedure written as a body.
#[test]
fn the_vllm_body_is_the_procedure() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut checked = 0;
    for dir in std::fs::read_dir(root.join("examples")).unwrap().flatten() {
        for f in std::fs::read_dir(dir.path()).unwrap().flatten() {
            let path = f.path();
            if path.extension().is_none_or(|e| e != "sq") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            if !src.contains("step {")
                || src.contains("serve exclusive")
                || src.contains("serve only")
            {
                continue;
            }
            let body = src.replace("step {", &format!("step {{ {VLLM} "));
            let ov = Overrides::default();
            let base = path.parent();
            let a = run_source(&src, &ov, base).unwrap_or_else(|e| panic!("{path:?}: {e}"));
            let b = run_source(&body, &ov, base).unwrap_or_else(|e| panic!("{path:?}: {e}"));
            assert_eq!(a.text(), b.text(), "{}", path.display());
            checked += 1;
        }
    }
    assert!(checked >= 10, "{checked} programs");
}

/// Three two-token prompts arrive together on a budget of eight. SGLang
/// prefills them in one batch, and decodes only when no prefill forms;
/// `exclusive prefill` takes one prefill an iteration.
#[test]
fn several_prefills_run_alone_in_one_iteration() {
    let prog = |serve: &str| {
        format!(
            r#"
            pool reqs {{ cap 8; admit via engine; }}
            pool kv {{ cap 100; }}
            stage engine : step {{ budget 8; cost 1; memory kv; {serve} }}
            workload {{ arrive batch(3); }}
            session {{
              set t0 = now;
              hold reqs (1), kv (4) {{
                prefill on engine (2) growing kv;
                observe ttft = now - t0;
                decode on engine (2) growing kv;
              }}
              end;
            }}
            run {{ horizon 20; warmup 0; seed 1; }}
            "#
        )
    };
    let sglang = "iteration { serve only (!decoding); admit; branch (tokens == 0) { serve; } }";
    let r = run(&prog(sglang));
    assert_eq!(
        r.observe("ttft").unwrap().samples,
        vec![1.0, 1.0, 1.0],
        "{}",
        r.text()
    );
    let r = run(&prog("serve exclusive prefill;"));
    assert_eq!(
        r.observe("ttft").unwrap().samples,
        vec![1.0, 2.0, 3.0],
        "{}",
        r.text()
    );
}

/// TensorRT-LLM's STATIC_BATCH: the waiting are admitted only into an
/// empty engine, so the second wave waits for the first to finish.
#[test]
fn a_gate_on_the_residents_admits_only_into_an_empty_engine() {
    let src = r#"
        pool reqs { cap 8; admit via engine; }
        stage engine : step {
          budget 64; cost 1;
          iteration { serve; branch (residents == 0) { admit; } }
        }
        stage gap : delay;
        workload { arrive batch(4); }
        session {
          run gap (serial < 2 ? 0 : 0.5);
          set t0 = now;
          hold reqs (1) {
            observe start = now;
            prefill on engine (1);
            decode on engine (4);
          }
          end;
        }
        run { horizon 50; warmup 0; seed 1; }
        "#;
    let r = run(src);
    let start = &r.observe("start").unwrap().samples;
    // the first two at 0; the two that arrive at 0.5 wait for an empty engine
    assert_eq!(start[..2], [0.0, 0.0], "{}", r.text());
    assert!(
        start[2] >= 5.0 && start[3] >= 5.0,
        "{start:?}\n{}",
        r.text()
    );
}

#[test]
fn a_body_that_may_schedule_nothing_does_not_link() {
    let prog = |body: &str| {
        format!(
            r#"
            pool reqs {{ cap 8; admit via engine; }}
            stage engine : step {{ budget 8; cost 1; {body} }}
            workload {{ arrive batch(1); }}
            session {{ hold reqs (1) {{ prefill on engine (2); }} end; }}
            run {{ horizon 20; }}
            "#
        )
    };
    let err = |body: &str| {
        compile_source(&prog(body), &Overrides::default())
            .err()
            .unwrap_or_else(|| panic!("`{body}` linked"))
    };
    assert!(
        err("iteration { branch (residents > 0) { serve; } }")
            .contains("neither serves nor admits")
    );
    assert!(err("iteration { serve; admit while (now < 5); }").contains("now"));
    assert!(err("iteration { serve; admit while (~bernoulli(0.5)); }").contains("draw"));
    assert!(
        err("iteration { serve; admit while (budget_left(engine) > 0); }").contains("budget_left")
    );
    assert!(err("serve only (decoding); iteration { serve; admit; }").contains("second"));
    assert!(err("serve exclusive prefill; iteration { serve; admit; }").contains("second"));
    // `admitted` and `preempted` are a body's
    let src = prog("iteration { serve; admit; }").replace(
        "prefill on engine (2);",
        "prefill on engine (2); set x = admitted;",
    );
    assert!(compile_source(&src, &Overrides::default()).is_err());
}
