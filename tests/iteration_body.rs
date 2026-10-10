//! An engine's schedule as a body (#355): `advance running`, `admit
//! waiting`, `branch` and `set` lower to the step's `iteration`, each
//! statement run once where written.

mod common;

use serq::{Overrides, compile_source, run_source};

fn run(src: &str, options: &Overrides) -> serq::Report {
    run_source(&common::main_source(src), options, None).unwrap()
}

/// A stage without a body runs vLLM's procedure: every program of the
/// corpus whose engines' schedule is the procedure gives the same report
/// with the procedure written as a body. An engine's schedule that is the
/// procedure lowers to no body; as both arms of a `branch` it is a body.
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
            let Some(body) = procedure_as_body(&src) else {
                continue;
            };
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

/// An engine's schedule `advance running …; admit waiting while
/// (running.preempted == 0) …;`, vLLM's procedure, as both arms of a
/// `branch`, which makes it a body; `None` for any other schedule.
fn procedure_as_body(src: &str) -> Option<String> {
    let i = src.find("advance running")?;
    let first = i + src[i..].find(';')? + 1;
    let rest = &src[first..];
    let admit = "admit waiting while (running.preempted == 0)";
    if !rest.trim_start().starts_with(admit) || src[i..first].contains(" only ") {
        return None;
    }
    let j = first + rest.find(admit)?;
    let end = j + src[j..].find(';')? + 1;
    Some(format!(
        "{}branch (running.count >= 0) {{ {p} }} else {{ {p} }}{}",
        &src[..i],
        &src[end..],
        p = &src[i..end]
    ))
}

/// Three two-token prompts arrive together on a budget of eight. SGLang
/// prefills them in one batch, and decodes only when no prefill forms;
/// `exclusive prefill` takes one prefill an iteration.
#[test]
fn several_prefills_run_alone_in_one_iteration() {
    let prog = |schedule: &str| {
        format!(
            r#"
        device gpu {{ kv cap 100; }}
        engine llm on gpu {{ reqs cap 8; tokens cap 8; schedule {{ {schedule} }} execute (1); }}
        pool reqs on llm {{ }}
        pool kv on gpu {{ }}
        workload {{ arrive batch(3);
          session {{ turn;
            end;

          }}
        }}
        server {{
          set t0 = now;
          hold reqs (cost(reqs, 1)), kv (cost(kv, 4)) {{
            run llm prefill (cost(llm, 2)) growing kv;
            observe ttft = now - t0;
            run llm decode (cost(llm, 2)) growing kv;
          }}
        }}

"#
        )
    };
    let sglang = "advance running only (!decoding); admit waiting; \
                  branch (batch.tokens == 0) { advance running; }";
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
        &prog("exclusive prefill; admit waiting while (running.preempted == 0);"),
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
        device gpu { }
        engine llm on gpu {
          reqs cap 8;
          tokens cap 64;
          schedule { advance running; branch (running.count == 0) { admit waiting; } }
          execute (1);
        }
        pool reqs on llm { }
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
            run llm prefill (cost(llm, 1));
            run llm decode (cost(llm, 4));
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
    let prog = |schedule: &str| {
        format!(
            r#"
        device gpu {{ }}
        engine llm on gpu {{ reqs cap 8; tokens cap 8; schedule {{ {schedule} }} execute (1); }}
        pool reqs on llm {{ }}
        workload {{ arrive batch(1);
          session {{ turn; end;
          }}
        }}
        server {{ hold reqs (cost(reqs, 1)) {{ run llm prefill (cost(llm, 2)); }}
        }}

"#
        )
    };
    let err = |schedule: &str| {
        compile_source(
            &common::main_source(&prog(schedule)),
            &common::horizon(20.0),
        )
        .err()
        .unwrap_or_else(|| panic!("`{schedule}` linked"))
    };
    assert!(
        err("branch (running.count > 0) { advance running; }")
            .contains("neither serves nor admits")
    );
    assert!(err("advance running; admit waiting while (now < 5);").contains("now"));
    assert!(err("advance running; admit waiting while (~bernoulli(0.5));").contains("draw"));
    assert!(
        err("advance running; admit waiting while (budget_left(llm) > 0);").contains("budget_left")
    );
    // `exclusive prefill` takes back decodes already chosen, which a body
    // cannot: it is refused in a schedule other than its own form, and in
    // the IR beside a body
    assert!(err("exclusive prefill; admit waiting;").contains("takes back"));
    let mut p = compile_source(
        &common::main_source(&prog("advance running; admit waiting;")),
        &common::horizon(20.0),
    )
    .unwrap();
    let serq::ir::CStageKind::Step(st) = &mut p.stages[0].kind else {
        panic!("engine is a step stage")
    };
    st.serve = serq::ir::CServe::ExclusivePrefill;
    let e = serq::Program::from_json(&p.to_json()).unwrap_err();
    assert!(e.contains("takes back"), "{e}");
    // `admitted` and `preempted` are a body's
    let src = prog("advance running; admit waiting;").replace(
        "run llm prefill (cost(llm, 2));",
        "run llm prefill (cost(llm, 2)); set x = admitted;",
    );
    assert!(compile_source(&common::main_source(&src), &common::horizon(20.0)).is_err());
}

/// The procedure and its body agree where serving is by keys and
/// preemption takes residents the iteration already served (their tokens
/// go back to the budget) or the grower itself, paths no example takes.
/// The body is the procedure as both arms of a `branch`, its order on each
/// `advance running` (`CIter::Serve`'s `by`) where the procedure's is the
/// step's.
#[test]
fn the_vllm_body_is_the_procedure_under_keys_and_preemption() {
    for order in [
        " by (-admission)",
        " decode first",
        " by (decoding ? remaining : -admission)",
        "",
    ] {
        for (kv, chunk, budget) in [(24, 4, 8), (24, 0, 16), (40, 4, 16)] {
            let cap = if chunk > 0 {
                format!(" each at most ({chunk})")
            } else {
                String::new()
            };
            let src = format!(
                r#"
        device gpu {{ kv cap {kv}; }}
        engine llm on gpu {{
          reqs cap 6;
          tokens cap {budget};
          schedule {{ advance running{order}{cap}; admit waiting while (running.preempted == 0){cap}; }}
          execute (1);
        }}
        pool reqs on llm {{ }}
        pool kv on gpu {{ preempt lifo; }}
        workload {{ arrive renewal(1.5);
          session {{ turn;
            end;

          }}
        }}
        server {{
          set n = 3 + serial - 5 * floor(serial / 5);
          hold reqs (cost(reqs, 1)), kv (cost(kv, 1)) {{
            run llm prefill (cost(llm, n)) growing kv;
            run llm decode (cost(llm, 6 + serial - 3 * floor(serial / 3))) growing kv;
          }}
        }}

"#
            );
            let options = Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(120.0)
            };
            let a = run(&src, &options);
            let b = run(&procedure_as_body(&src).unwrap(), &options);
            if kv == 24 {
                assert!(
                    a.pool("kv").unwrap().preemptions > 0,
                    "{order}\n{}",
                    a.text()
                );
            }
            assert_eq!(
                a.text(),
                b.text(),
                "{order} kv {kv} chunk {chunk} budget {budget}"
            );
        }
    }
}

/// A schedule whose `advance running` and `admit waiting` share an `only
/// (p)` lowers to the body `[Serve only p, Admit only p while !preempted]`.
/// That the body runs as the shared `only` ran before it was lowered to one
/// (a stage option then), was shown in #362
/// on this program and the two FasterTransformer ones; paths they do not
/// take (a preemption beside `only`, `only` with `serve by`) were compared
/// by reading the code, not by a run.
#[test]
fn an_engines_shared_only_lowers_to_the_serve_only_body() {
    use serq::ir::{CExpr, CIter, CStageKind, CtxVar, UnOp};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let file = "examples/papers/bari_rad.sq";
    let path = root.join(file);
    let text = std::fs::read_to_string(&path).unwrap();
    let p = serq::compile_file(&common::main_source(&text), &path, &common::horizon(20.0))
        .unwrap_or_else(|e| panic!("{file}: {e}"));
    let CStageKind::Step(st) = &p.stages[0].kind else {
        panic!("{file}: `E` is an engine")
    };
    let Some(
        [
            CIter::Serve {
                only: Some(serve),
                by: None,
            },
            CIter::Admit {
                only: Some(admit),
                gate: Some(gate),
            },
        ],
    ) = st.iteration.as_deref()
    else {
        panic!("{file}: {:?}", st.iteration)
    };
    assert_eq!(serve, admit, "{file}");
    assert_eq!(
        *gate,
        CExpr::Unary(UnOp::Not, Box::new(CExpr::Ctx(CtxVar::Preempted))),
        "{file}"
    );
}

/// A body the linker cannot see stall still says so: the engine that ends
/// the run with work and a last try that scheduled nothing is named.
#[test]
fn an_engine_idle_with_work_is_named() {
    let src = r#"
        device gpu { }
        engine llm on gpu {
          reqs cap 8;
          tokens cap 8;
          schedule { advance running; admit waiting while (batch.tokens > 0); }
          execute (1);
        }
        pool reqs on llm { }
        workload { arrive batch(3);
          session { turn; end;
          }
        }
        server { hold reqs (cost(reqs, 1)) { run llm prefill (cost(llm, 2)); }
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
    assert!(r.text().contains("idle: stage `llm`"), "{}", r.text());
}

/// A guard is a test: a value other than 1 or 0 fails the run.
#[test]
fn a_guard_that_is_not_a_test_fails_the_run() {
    let src = r#"
        device gpu { }
        engine llm on gpu {
          reqs cap 8;
          tokens cap 8;
          schedule { advance running; admit waiting while (running.count + 2); }
          execute (1);
        }
        pool reqs on llm { }
        workload { arrive batch(1);
          session { turn; end;
          }
        }
        server { hold reqs (cost(reqs, 1)) { run llm prefill (cost(llm, 2)); }
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
    let prog = |items: &str| {
        format!(
            r#"
        device gpu {{ }}
        engine llm on gpu {{ reqs cap 64; tokens cap 64; {items} execute (1); }}
        pool reqs on llm {{ }}
        workload {{ arrive renewal(1);
          session {{ turn;
            end;

          }}
        }}
        server {{
          set t0 = now;
          hold reqs (cost(reqs, 1)) {{
            observe wait = now - t0;
            run llm prefill (cost(llm, 1));
            run llm decode (cost(llm, 30));
          }}
        }}

"#
        )
    };
    let wait = |items: &str| {
        let r = run(
            &prog(items),
            &Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(40.0)
            },
        );
        r.observe("wait").unwrap().samples.clone()
    };
    let every = wait("schedule { advance running; admit waiting while (running.preempted == 0); }");
    assert!(every.iter().all(|&w| w == 0.0), "{every:?}");
    let alternate = wait(
        "state just = 0; \
         schedule { \
           advance running; branch (just == 0) { admit waiting; } \
           set just = waiting.admitted > 0; \
         }",
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
        device gpu { }
        engine llm on gpu {
          reqs cap 1;
          tokens cap 4;
          state n = 0;
          schedule {
            set n = n + 1;
            advance running only (decoding);
            admit waiting while (running.count == 0);
          }
          execute (1);
        }
        pool reqs on llm { }
        workload { arrive renewal(1);
          session { turn; end;
          }
        }
        server { hold reqs (cost(reqs, 1)) { run llm prefill (cost(llm, 100)); }
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
    let prog = |items: &str, session: &str| {
        format!(
            r#"
        device gpu {{ }}
        device tpu {{ }}
        engine llm on gpu {{ reqs cap 8; tokens cap 8; {items} execute (1); }}
        pool reqs on llm {{ }}
        engine other on tpu {{
          tokens cap 8;
          state r = 0;
          schedule {{ advance running; admit waiting; }}
          execute (1);
        }}
        workload {{ arrive batch(1);
          session {{ turn; end;
          }}
        }}
        server {{ hold reqs (cost(reqs, 1)) {{ run llm prefill (cost(llm, 2)); }} {session}
        }}

"#
        )
    };
    let err = |items: &str, session: &str| {
        compile_source(
            &common::main_source(&prog(items, session)),
            &common::horizon(20.0),
        )
        .err()
        .unwrap_or_else(|| panic!("`{items}` `{session}` linked"))
    };
    let body = "state k = 0; schedule { advance running; admit waiting; set k = k + 1; }";
    assert!(
        compile_source(
            &common::main_source(&prog(body, "")),
            &common::horizon(20.0)
        )
        .is_ok()
    );
    assert!(err("state k = 0; schedule { advance running; admit waiting while (running.preempted == 0); }", "").contains("is set by nothing"));
    // an `only` is a body, but none that sets the register
    assert!(
        err(
            "state k = 0; schedule { advance running only (decoding); \
             admit waiting only (decoding) while (running.preempted == 0); }",
            ""
        )
        .contains("is set by nothing")
    );
    assert!(
        err(
            "schedule { advance running; admit waiting; set r = 1; }",
            ""
        )
        .contains("another stage")
    );
    assert!(err(body, "set x = k;").contains("register"));
    assert!(
        err(
            "state cached = 0; schedule { advance running; admit waiting; }",
            ""
        )
        .contains("taken")
    );
    assert!(
        err(
            "state k = 0; schedule { advance running; admit waiting; set k = now; }",
            ""
        )
        .contains("now")
    );
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
        device gpu {{ }}
        engine b on gpu {{
          reqs cap 8;
          tokens cap 8;
          state go = 0;
          schedule {{ advance running; admit waiting; set go = 1; }}
          execute (1);
        }}
        pool reqs on b {{ }}
        pool other {{ cap 8; }}
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
            "device tpu { }
        engine a on tpu {
          tokens cap 8;
          schedule {
            advance running only (go == 1);
            admit waiting only (go == 1) while (running.preempted == 0);
          }
          execute (1);
        }",
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
    let arr = "device tpu[2] { }
        engine c[2] on tpu {
          tokens cap 8;
          state q = 0;
          schedule { advance running; admit waiting; }
          execute (1);
        }";
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
        device gpu { }
        engine llm on gpu {
          reqs cap 4;
          tokens cap 8;
          state k = 0;
          schedule {
            advance running only (decoding);
            set k = k + 1;
            admit waiting only (decoding);
          }
          execute (1);
        }
        pool reqs on llm { }
        workload { arrive batch(2);
          session { turn; end;
          }
        }
        server { hold reqs (cost(reqs, 1)) { run llm prefill (cost(llm, 2)); }
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
        device gpu { kv cap 1000; }
        engine llm on gpu {
          reqs cap 16;
          tokens cap 512;
          state r = 5000;
          schedule { set r = 10; advance running; admit waiting; }
          execute (0.001 + batch.tokens * 1e-5);
        }
        pool reqs on llm { }
        pool kv on gpu { evict lru; }
        workload { arrive renewal(2);
          session { turn;
            end;

          }
        }
        server {
          hold reqs (cost(reqs, 1)), kv (cost(kv, 100)) reserve (cost(kv, 100 + r)) {
            run llm prefill (cost(llm, 100));
            run llm decode (cost(llm, 9)) growing kv;
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
