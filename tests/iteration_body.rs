//! A step stage's iteration as the program writes it (`iteration { … }`,
//! #355): `serve`, `admit` and `branch`, run once each where written.

mod common;

use serq::{Overrides, compile_source, run_source};

fn run(src: &str, options: &Overrides) -> serq::Report {
    run_source(&common::main_source(src), options, None).unwrap()
}

/// vLLM's procedure, written out.
const VLLM: &str = "iteration { serve; admit while (!preempted); }";

/// A stage without a body runs vLLM's procedure: every program of the
/// corpus whose engines have no `exclusive prefill`, no `serve only` and no body
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
                || src.contains("iteration {")
            {
                continue;
            }
            let body = src.replace("step {", &format!("step {{ {VLLM} "));
            let ov = common::horizon(20.0);
            let base = path.parent();
            let a = run_source(&common::main_source(&src), &ov, base)
                .unwrap_or_else(|e| panic!("{path:?}: {e}"));
            let b = run_source(&common::main_source(&body), &ov, base)
                .unwrap_or_else(|e| panic!("{path:?}: {e}"));
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
        workload {{ arrive batch(3);
          session {{ turn;
            end;

          }}
        }}
        server {{
          set t0 = now;
          hold reqs (cost(reqs, 1)), kv (cost(kv, 4)) {{
            run engine prefill (cost(engine, 2)) growing kv;
            observe ttft = now - t0;
            run engine decode (cost(engine, 2)) growing kv;
          }}
        }}

"#
        )
    };
    let sglang = "iteration { serve only (!decoding); admit; branch (tokens == 0) { serve; } }";
    let r = run(
        &prog(sglang),
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(20.0)
        },
    );
    assert_eq!(
        r.observe("ttft").unwrap().samples,
        vec![1.0, 1.0, 1.0],
        "{}",
        r.text()
    );
    let r = run(
        &prog("serve exclusive prefill;"),
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(20.0)
        },
    );
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
        workload { arrive batch(4);
          session { turn;
            end;

          }
        }
        server {
          run gap (cost(gap, serial < 2 ? 0 : 0.5));
          set t0 = now;
          hold reqs (cost(reqs, 1)) {
            observe start = now;
            run engine prefill (cost(engine, 1));
            run engine decode (cost(engine, 4));
          }
        }

"#;
    let r = run(
        src,
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(50.0)
        },
    );
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
        workload {{ arrive batch(1);
          session {{ turn; end;
          }}
        }}
        server {{ hold reqs (cost(reqs, 1)) {{ run engine prefill (cost(engine, 2)); }}
        }}

"#
        )
    };
    let err = |body: &str| {
        compile_source(&common::main_source(&prog(body)), &common::horizon(20.0))
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
    assert!(err("serve only (decoding); iteration { serve; admit; }").contains("two bodies"));
    assert!(err("serve exclusive prefill; iteration { serve; admit; }").contains("takes back"));
    // `admitted` and `preempted` are a body's
    let src = prog("iteration { serve; admit; }").replace(
        "run engine prefill (cost(engine, 2));",
        "run engine prefill (cost(engine, 2)); set x = admitted;",
    );
    assert!(compile_source(&common::main_source(&src), &common::horizon(20.0)).is_err());
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
        workload {{ arrive renewal(1.5);
          session {{ turn;
            end;

          }}
        }}
        server {{
          set n = 3 + serial - 5 * floor(serial / 5);
          hold reqs (cost(reqs, 1)), kv (cost(kv, 1)) {{
            run engine prefill (cost(engine, n)) growing kv;
            run engine decode (cost(engine, 6 + serial - 3 * floor(serial / 3))) growing kv;
          }}
        }}

"#
                )
            };
            let a = run(
                &prog(""),
                &Overrides {
                    warmup: Some(0.0),
                    seed: Some(1),
                    ..common::horizon(120.0)
                },
            );
            let b = run(
                &prog(VLLM),
                &Overrides {
                    warmup: Some(0.0),
                    seed: Some(1),
                    ..common::horizon(120.0)
                },
            );
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
/// while (!preempted);`: the linker writes the one as the other, so the
/// two compile to one IR. That the body runs as the stage option ran,
/// before it was lowered, was shown on these three programs in #362; paths
/// they do not take (a preemption beside `only`, `only` with `serve by`)
/// were compared by reading the code, not by a run.
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
        let ov = common::horizon(20.0);
        let ir = |s: &str| {
            serq::compile_file(&common::main_source(s), &path, &ov)
                .unwrap_or_else(|e| panic!("{file}: {e}"))
                .to_json()
        };
        assert_eq!(ir(&src), ir(&body), "{file}");
    }
}

/// A body the linker cannot see stall still says so: the engine that ends
/// the run with work and a last try that scheduled nothing is named.
#[test]
fn an_engine_idle_with_work_is_named() {
    let src = r#"
        pool reqs { cap 8; admit via engine; }
        stage engine : step { budget 8; cost 1; iteration { serve; admit while (tokens > 0); } }
        workload { arrive batch(3);
          session { turn; end;
          }
        }
        server { hold reqs (cost(reqs, 1)) { run engine prefill (cost(engine, 2)); }
        }

"#;
    let r = run(
        src,
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(20.0)
        },
    );
    assert!(r.stages[0].idle_with_work, "{}", r.text());
    assert!(r.text().contains("idle: stage `engine`"), "{}", r.text());
}

/// A guard is a test: a value other than 1 or 0 fails the run.
#[test]
fn a_guard_that_is_not_a_test_fails_the_run() {
    let src = r#"
        pool reqs { cap 8; admit via engine; }
        stage engine : step { budget 8; cost 1; iteration { serve; admit while (residents + 2); } }
        workload { arrive batch(1);
          session { turn; end;
          }
        }
        server { hold reqs (cost(reqs, 1)) { run engine prefill (cost(engine, 2)); }
        }

"#;
    let e = run_source(
        &common::main_source(src),
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(20.0)
        },
        None,
    )
    .unwrap_err();
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
        workload {{ arrive renewal(1);
          session {{ turn;
            end;

          }}
        }}
        server {{
          set t0 = now;
          hold reqs (cost(reqs, 1)) {{
            observe wait = now - t0;
            run engine prefill (cost(engine, 1));
            run engine decode (cost(engine, 30));
          }}
        }}

"#
        )
    };
    let wait = |body: &str| {
        let r = run(
            &prog(body),
            &Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(40.0)
            },
        );
        r.observe("wait").unwrap().samples.clone()
    };
    let every = wait("");
    assert!(every.iter().all(|&w| w == 0.0), "{every:?}");
    let alternate = wait(
        "state just = 0; \
         iteration { serve; branch (just == 0) { admit; } set just = admitted > 0; }",
    );
    // the first arrives at 1 and is admitted (`just` 1); the one of 2 waits,
    // since that iteration does not admit (`just` 0); at 3 it is admitted
    // with the one of 3, and so on: by session 0, 1, 0, 1, …
    assert_eq!(
        alternate[..7],
        [0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0],
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
        workload { arrive renewal(1);
          session { turn; end;
          }
        }
        server { hold reqs (cost(reqs, 1)) { run engine prefill (cost(engine, 100)); }
        }
        gauge count = n;

"#;
    let r = run(
        src,
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(30.0)
        },
    );
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
        workload {{ arrive batch(1);
          session {{ turn; end;
          }}
        }}
        server {{ hold reqs (cost(reqs, 1)) {{ run engine prefill (cost(engine, 2)); }} {session}
        }}

"#
        )
    };
    let err = |stage: &str, session: &str| {
        compile_source(
            &common::main_source(&prog(stage, session)),
            &common::horizon(20.0),
        )
        .err()
        .unwrap_or_else(|| panic!("`{stage}` `{session}` linked"))
    };
    let body = "state k = 0; iteration { serve; admit; set k = k + 1; }";
    assert!(
        compile_source(
            &common::main_source(&prog(body, "")),
            &common::horizon(20.0)
        )
        .is_ok()
    );
    assert!(err("state k = 0;", "").contains("nothing sets"));
    // a stage `serve only` is a body, but none that sets the register
    assert!(err("state k = 0; serve only (decoding);", "").contains("nothing sets"));
    assert!(err("iteration { serve; admit; set r = 1; }", "").contains("another stage"));
    assert!(err(body, "set x = k;").contains("register"));
    assert!(err("state cached = 0; iteration { serve; admit; }", "").contains("taken"));
    assert!(err("state k = 0; iteration { serve; admit; set k = now; }", "").contains("now"));
}

/// A register is read where its stage orders the read. Read by another
/// stage, a `ps` capacity, or a hold its stage does not admit, the read and
/// the set fall at one instant in the order of the declarations, and the
/// review of #367 found the answer moving with that order; such a program
/// does not link. A stage array has no register, and a claim's `given` is
/// the session's.
#[test]
fn a_register_is_read_where_its_stage_orders_the_read() {
    let prog = |extra: &str, hold: &str| {
        format!(
            r#"
        pool reqs {{ cap 8; admit via b; }}
        pool other {{ cap 8; }}
        stage b : step {{ budget 8; cost 1; state go = 0; iteration {{ serve; admit; set go = 1; }} }}
        {extra}
        workload {{ arrive batch(1);
          session {{ turn; end;
          }}
        }}
        server {{ {hold}
        }}

"#
        )
    };
    let ok = |extra: &str, hold: &str| {
        compile_source(
            &common::main_source(&prog(extra, hold)),
            &common::horizon(20.0),
        )
    };
    let base = "hold reqs (cost(reqs, 1)) { run b prefill (cost(b, 2)); }";
    // its pool's header and keys, a gauge
    assert!(
        ok(
            "gauge g = go;",
            "hold reqs (cost(reqs, 1 + go)) { run b prefill (cost(b, 2)); }"
        )
        .is_ok()
    );
    // a hold whose first pool, where it waits, the stage admits, whatever
    // else it holds (SGLang's `reqs` and `kv`); not one that waits elsewhere
    assert!(
        ok(
            "",
            "hold reqs (cost(reqs, 1)), other (cost(other, 1 + go)) { run b prefill (cost(b, 2)); }"
        )
        .is_ok()
    );
    let e = ok(
        "",
        "hold other (cost(other, 1)), reqs (cost(reqs, 1 + go)) { run b prefill (cost(b, 2)); }",
    )
    .unwrap_err();
    assert!(e.contains("`go` is stage `b`'s register"), "{e}");
    // a ps capacity, another stage, a hold on a pool admitted at settle time
    for (extra, hold) in [
        ("stage p : ps(1 + go);", base),
        (
            "stage a : step { budget 8; cost 1; serve only (go == 1); }",
            base,
        ),
        (
            "",
            "hold other (cost(other, 1 + go)) { run b prefill (cost(b, 2)); }",
        ),
    ] {
        let e = ok(extra, hold)
            .err()
            .unwrap_or_else(|| panic!("{extra} {hold} linked"));
        assert!(e.contains("`go` is stage `b`'s register"), "{e}");
    }
    let arr = "stage c[2] : step { budget 8; cost 1; state q = 0; iteration { serve; admit; } }";
    assert!(ok(arr, base).unwrap_err().contains("stage array"));
    let given = "claim c given (go == 0): at end (1);";
    assert!(ok(given, base).is_err());
}

/// A try that admitted is kept with its sets, though it scheduled nothing
/// (the newcomer excluded): the admission stays, and so does what the body
/// set.
#[test]
fn a_try_that_admitted_keeps_its_sets() {
    let src = r#"
        pool reqs { cap 4; admit via engine; }
        stage engine : step {
          budget 8; cost 1;
          state k = 0;
          iteration { serve only (decoding); set k = k + 1; admit only (decoding); }
        }
        workload { arrive batch(2);
          session { turn; end;
          }
        }
        server { hold reqs (cost(reqs, 1)) { run engine prefill (cost(engine, 2)); }
        }
        gauge seen = k;

"#;
    let r = run(
        src,
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(10.0)
        },
    );
    assert!(r.pool("reqs").unwrap().admissions > 0, "{}", r.text());
    assert!(r.gauge("seen").unwrap().max >= 1.0, "{}", r.text());
}

/// A reserve that reads a register moves at the stage's iterations, so
/// joining the queue does not judge it (the review of #377: a reserve of
/// `prompt + r` with `r` 5000 at the join, lowered to 10 by the first
/// iteration, rejected all 101 sessions).
#[test]
fn a_reserve_on_a_register_waits_for_the_iteration() {
    let src = r#"
        pool reqs { cap 16; admit via engine; }
        pool kv { cap 1000; evict lru; }
        stage engine : step {
          budget 512; cost 0.001 + tokens * 1e-5; memory kv;
          state r = 5000;
          iteration { set r = 10; serve; admit; }
        }
        workload { arrive renewal(2);
          session { turn;
            end;

          }
        }
        server {
          hold reqs (cost(reqs, 1)), kv (cost(kv, 100)) reserve (cost(kv, 100 + r)) {
            run engine prefill (cost(engine, 100));
            run engine decode (cost(engine, 9)) growing kv;
          }
        }

"#;
    let r = run(
        src,
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(50.0)
        },
    );
    assert_eq!(r.pool("kv").unwrap().rejected, 0, "{}", r.text());
    assert!(r.pool("reqs").unwrap().admissions > 20, "{}", r.text());
}
