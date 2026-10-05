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
    // the first two at 0; they prefill 1 and decode 4, one token an
    // iteration of cost 1, so the engine is empty at 5, and the two that
    // arrived at 0.5 are admitted then, not at 1
    assert_eq!(start, &[0.0, 0.0, 5.0, 5.0], "{}", r.text());
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

/// The procedure and its body agree where serving is by keys and
/// preemption takes residents the iteration already served (their tokens
/// go back to the budget) or the grower itself, paths no example takes.
#[test]
fn the_vllm_body_is_the_procedure_under_keys_and_preemption() {
    for serve in [
        "serve by (-admission);",
        "serve decode first;",
        "serve by (decoding ? remaining : -admission);",
        "",
    ] {
        for (kv, chunk, budget) in [(24, 4, 8), (24, 0, 16), (40, 4, 16)] {
            let prog = |body: &str| {
                format!(
                    r#"
                    pool reqs {{ cap 6; admit via engine; }}
                    pool kv {{ cap {kv}; preempt lifo; }}
                    stage engine : step {{
                      budget {budget}; chunk {chunk}; cost 1; memory kv; {serve} {body}
                    }}
                    workload {{ arrive renewal(1.5); }}
                    session {{
                      set n = 3 + serial - 5 * floor(serial / 5);
                      hold reqs (1), kv (1) {{
                        prefill on engine (n) growing kv;
                        decode on engine (6 + serial - 3 * floor(serial / 3)) growing kv;
                      }}
                      end;
                    }}
                    run {{ horizon 120; warmup 0; seed 1; }}
                    "#
                )
            };
            let a = run(&prog(""));
            let b = run(&prog(VLLM));
            if kv == 24 {
                assert!(
                    a.pool("kv").unwrap().preemptions > 0,
                    "{serve}\n{}",
                    a.text()
                );
            }
            assert_eq!(
                a.text(),
                b.text(),
                "{serve} kv {kv} chunk {chunk} budget {budget}"
            );
        }
    }
}

/// The stage's `serve only (p)` is the body `serve only (p); admit only (p)
/// while (!preempted);`: a newcomer `p` excludes is admitted and waits.
#[test]
fn a_stage_only_is_a_body() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (file, p) in [
        (
            "examples/single-turn/fastertransformer.sq",
            "decoders > 0 ? decoding : !decoding",
        ),
        (
            "examples/papers/dai_fastertransformer.sq",
            "decoders > 0 ? decoding : !decoding",
        ),
        (
            "examples/papers/bari_rad.sq",
            "decoders >= bcol || decoders == residents ? decoding : !decoding",
        ),
    ] {
        let path = root.join(file);
        let src = std::fs::read_to_string(&path).unwrap();
        let stage = format!("serve only ({p});");
        assert!(src.contains(&stage), "{file}");
        let body = src.replace(
            &stage,
            &format!("iteration {{ serve only ({p}); admit only ({p}) while (!preempted); }}"),
        );
        let ov = Overrides::default();
        let a = run_source(&src, &ov, path.parent()).unwrap();
        let b = run_source(&body, &ov, path.parent()).unwrap();
        assert_eq!(a.text(), b.text(), "{file}");
    }
}

/// A body the linker cannot see stall still says so: the engine that ends
/// the run with work and a last try that scheduled nothing is named.
#[test]
fn an_engine_idle_with_work_is_named() {
    let src = r#"
        pool reqs { cap 8; admit via engine; }
        stage engine : step { budget 8; cost 1; iteration { serve; admit while (tokens > 0); } }
        workload { arrive batch(3); }
        session { hold reqs (1) { prefill on engine (2); } end; }
        run { horizon 20; warmup 0; seed 1; }
        "#;
    let r = run(src);
    assert!(r.stages[0].idle_with_work, "{}", r.text());
    assert!(r.text().contains("idle: stage `engine`"), "{}", r.text());
}

/// A guard is a test: a value other than 1 or 0 fails the run.
#[test]
fn a_guard_that_is_not_a_test_fails_the_run() {
    let src = r#"
        pool reqs { cap 8; admit via engine; }
        stage engine : step { budget 8; cost 1; iteration { serve; admit while (residents + 2); } }
        workload { arrive batch(1); }
        session { hold reqs (1) { prefill on engine (2); } end; }
        run { horizon 20; warmup 0; seed 1; }
        "#;
    let e = run_source(src, &Overrides::default(), None).unwrap_err();
    assert!(e.contains("a test is 1 or 0"), "{e}");
}

/// TGI with chunking admits in a forward and not in the next one
/// (`backend.rs` L221-L236, L285-L288 at b4adbf2f): a register remembers
/// whether the last iteration admitted. One request arrives each second,
/// an iteration lasts one: without the register each is admitted at once,
/// with it every other one waits a second.
#[test]
fn a_register_remembers_the_last_iteration() {
    let prog = |body: &str| {
        format!(
            r#"
            pool reqs {{ cap 64; admit via engine; }}
            stage engine : step {{ budget 64; cost 1; {body} }}
            workload {{ arrive renewal(1); }}
            session {{
              set t0 = now;
              hold reqs (1) {{
                observe wait = now - t0;
                prefill on engine (1);
                decode on engine (30);
              }}
              end;
            }}
            run {{ horizon 40; warmup 0; seed 1; }}
            "#
        )
    };
    let wait = |body: &str| {
        let r = run(&prog(body));
        r.observe("wait").unwrap().samples.clone()
    };
    let every = wait("");
    assert!(every.iter().all(|&w| w == 0.0), "{every:?}");
    let alternate = wait(
        "state just = 0; \
         iteration { serve; branch (just == 0) { admit; } set just = admitted > 0; }",
    );
    assert!(
        alternate.contains(&1.0) && alternate.contains(&0.0),
        "{alternate:?}"
    );
    assert!(
        alternate.iter().all(|&w| w == 0.0 || w == 1.0),
        "{alternate:?}"
    );
}

/// A register's `set` takes effect with its iteration: an engine whose
/// body schedules nothing has had no iteration, and the count of
/// iterations a register keeps is the stage's.
#[test]
fn a_set_in_an_iteration_that_is_none_is_undone() {
    let src = r#"
        pool reqs { cap 1; admit via engine; }
        stage engine : step {
          budget 4; cost 1;
          state n = 0;
          iteration { set n = n + 1; serve only (decoding); admit while (residents == 0); }
        }
        workload { arrive renewal(1); }
        session { hold reqs (1) { prefill on engine (100); } end; }
        gauge count = n;
        run { horizon 30; warmup 0; seed 1; }
        "#;
    let r = run(src);
    // the first session is admitted and gets 4 tokens; after that its
    // prefill is excluded and nothing else may be admitted: every later
    // arrival is an event and a try, and none is an iteration
    assert_eq!(r.gauge("count").unwrap().max, 1.0, "{}", r.text());
}

#[test]
fn a_register_is_the_stage_s_own() {
    let prog = |stage: &str, session: &str| {
        format!(
            r#"
            pool reqs {{ cap 8; admit via engine; }}
            stage engine : step {{ budget 8; cost 1; {stage} }}
            stage other : step {{ budget 8; cost 1; state r = 0; iteration {{ serve; admit; }} }}
            workload {{ arrive batch(1); }}
            session {{ hold reqs (1) {{ prefill on engine (2); }} {session} end; }}
            run {{ horizon 20; }}
            "#
        )
    };
    let err = |stage: &str, session: &str| {
        compile_source(&prog(stage, session), &Overrides::default())
            .err()
            .unwrap_or_else(|| panic!("`{stage}` `{session}` linked"))
    };
    let body = "state k = 0; iteration { serve; admit; set k = k + 1; }";
    assert!(compile_source(&prog(body, ""), &Overrides::default()).is_ok());
    assert!(err("state k = 0;", "").contains("nothing sets"));
    assert!(err("iteration { serve; admit; set r = 1; }", "").contains("another stage"));
    assert!(err(body, "set x = k;").contains("register"));
    assert!(err("state cached = 0; iteration { serve; admit; }", "").contains("taken"));
    assert!(err("state k = 0; iteration { serve; admit; set k = now; }", "").contains("now"));
}
