//! An engine's schedule as a body (#355): `advance running`, `admit
//! waiting`, `if` and `set` lower to the step's `iteration`, each
//! statement run once where written.

mod common;

use serq::Overrides;

/// A workload's text and the overrides it runs with.
type Pair = (String, Overrides);

fn run(src: &str, (workload, options): &Pair) -> serq::Report {
    common::run(&common::main_source(src), workload, options).unwrap()
}

/// `src` (a deployment) compiled with the pair's workload.
fn compile(src: &str, (workload, options): &Pair) -> Result<serq::Program, String> {
    common::compile(src, workload, options)
}

/// `ov`, with a workload of single-turn sessions that arrive by `arrive`.
fn sessions(arrive: &str, ov: Overrides) -> Pair {
    (
        format!("workload {{ arrive {arrive};\n  session {{ turn; end;\n  }}\n}}\n"),
        ov,
    )
}

/// `scheduler` with `taking {pools}` on each `admit waiting`: what the
/// engine's request is admitted with.
fn taking(schedule: &str, pools: &str) -> String {
    let mut out = String::new();
    let mut rest = schedule;
    while let Some(i) = rest.find("admit waiting") {
        let end = i + rest[i..].find(';').unwrap();
        out.push_str(&rest[..end]);
        out.push_str(" taking ");
        out.push_str(pools);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// A stage without a body runs vLLM's procedure: every program of the
/// corpus whose engines' schedule is the procedure gives the same report
/// with the procedure written as a body. An engine's schedule that is the
/// procedure lowers to no body; as both arms of an `if` it is a body.
#[test]
fn the_vllm_body_is_the_procedure() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut checked = 0;
    for dir in std::fs::read_dir(root.join("examples")).unwrap().flatten() {
        // (`examples/pairs.json` is a file beside the groups)
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().is_none_or(|e| e != "sq") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            let Some(body) = procedure_as_body(&src) else {
                continue;
            };
            // (a workload's file has no engine: it is skipped above)
            let ov = common::horizon(20.0);
            let w = serq::paired_workload(&path)
                .unwrap_or_else(|| panic!("{}: no workload", path.display()));
            let workload = std::fs::read_to_string(&w).unwrap();
            let names = (path.to_str().unwrap(), w.to_str().unwrap());
            let run = |src: &str| {
                serq::run_paired(&common::main_source(src), &workload, names, &ov, w.parent())
                    .unwrap_or_else(|e| panic!("{path:?}: {e}"))
            };
            let (a, b) = (run(&src), run(&body));
            assert_eq!(a.text(), b.text(), "{}", path.display());
            checked += 1;
        }
    }
    assert!(checked >= 10, "{checked} programs");
}

/// An engine's schedule `advance running …; admit waiting while
/// (running.preempted == 0) …;`, vLLM's procedure, as both arms of a
/// `if`, which makes it a body; `None` for any other schedule.
fn procedure_as_body(src: &str) -> Option<String> {
    let i = src.find("advance running")?;
    let first = i + src[i..].find(';')? + 1;
    let rest = &src[first..];
    let admit = "admit waiting while (running.preempted == 0)";
    // the comments between the two statements say what `admit` takes
    let next = rest
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("//"))?;
    if !next.starts_with(admit) || src[i..first].contains(" only ") {
        return None;
    }
    let j = first + rest.find(admit)?;
    let end = j + src[j..].find(';')? + 1;
    Some(format!(
        "{}if (running.count >= 0) {{ {p} }} else {{ {p} }}{}",
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
        let schedule = taking(schedule, "reqs (1), kv (4)");
        format!(
            r#"
        device gpu {{ kv cap 100; }}
        engine llm on gpu {{
          reqs cap 8;
          tokens cap 8;
          scheduler {{ {schedule} }}
          worker {{ forward (1); }}
        }}
        pool reqs on llm {{ }}
        pool kv on gpu {{ }}
        server {{
          set t0 = now;
          llm (2, 3);
          observe ttft = llm.first - t0;
        }}

"#
        )
    };
    let sglang = "advance running only (!decoding); admit waiting; \
                  if (batch.tokens == 0) { advance running; }";
    let r = run(
        &prog(sglang),
        &sessions(
            "batch(3)",
            Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(20.0)
            },
        ),
    );
    assert_eq!(
        r.observe("ttft").unwrap().samples,
        vec![1.0, 1.0, 1.0],
        "{}",
        r.text()
    );
    let r = run(
        &prog("exclusive prefill; admit waiting while (running.preempted == 0);"),
        &sessions(
            "batch(3)",
            Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(20.0)
            },
        ),
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
          scheduler { advance running; if (running.count == 0) { admit waiting taking reqs (1); } }
          worker { forward (1); }
        }
        pool reqs on llm { }
        stage gap : delay;
        server {
          run gap (cost(gap, serial < 2 ? 0 : 0.5));
          set t0 = now;
          llm (1, 5);
          observe start = llm.admitted;
        }

"#;
    let r = run(
        src,
        &sessions(
            "batch(4)",
            Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(50.0)
            },
        ),
    );
    let start = &r.observe("start").unwrap().samples;
    // the first two at 0; they prefill 1 and decode 4, one token an
    // iteration of cost 1, so the engine is empty at 5, and the two that
    // arrived at 0.5 are admitted then, not at 1
    assert_eq!(start, &[0.0, 0.0, 5.0, 5.0], "{}", r.text());
}

#[test]
fn a_body_that_may_schedule_nothing_does_not_link() {
    // (a schedule without `admit waiting` has no `taking`: its request
    // holds nothing)
    let prog = |schedule: &str| {
        let schedule = taking(schedule, "reqs (1)");
        format!(
            r#"
        device gpu {{ }}
        engine llm on gpu {{
          reqs cap 8;
          tokens cap 8;
          scheduler {{ {schedule} }}
          worker {{ forward (1); }}
        }}
        pool reqs on llm {{ }}
        server {{ llm (2, 1);
        }}

"#
        )
    };
    let err = |schedule: &str| {
        compile(
            &common::main_source(&prog(schedule)),
            &sessions("batch(1)", common::horizon(20.0)),
        )
        .err()
        .unwrap_or_else(|| panic!("`{schedule}` linked"))
    };
    assert!(
        err("if (running.count > 0) { advance running; }")
            .contains("neither advances `running` nor admits `waiting`")
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
    let mut p = compile(
        &common::main_source(&prog("advance running; admit waiting;")),
        &sessions("batch(1)", common::horizon(20.0)),
    )
    .unwrap();
    let serq::ir::CStageKind::Step(st) = &mut p.stages[0].kind else {
        panic!("engine is a step stage")
    };
    st.serve = serq::ir::CServe::ExclusivePrefill;
    let e = serq::Program::from_json(&p.to_json()).unwrap_err();
    assert!(e.contains("takes back"), "{e}");
    // `admitted` and `preempted` are a body's
    let src = prog("advance running; admit waiting;")
        .replace("llm (2, 1);", "llm (2, 1); set x = admitted;");
    assert!(
        compile(
            &common::main_source(&src),
            &sessions("batch(1)", common::horizon(20.0))
        )
        .is_err()
    );
}

/// The procedure and its body agree where serving is by keys and
/// preemption takes residents the iteration already served (their tokens
/// go back to the budget) or the grower itself, paths no example takes.
/// The body is the procedure as both arms of an `if`, its order on each
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
          scheduler {{
            advance running{order}{cap};
            admit waiting while (running.preempted == 0){cap} taking reqs (1), kv (1);
          }}
          worker {{ forward (1); }}
        }}
        pool reqs on llm {{ }}
        pool kv on gpu {{ preempt lifo; }}
        server {{
          set n = 3 + serial - 5 * floor(serial / 5);
          llm (n, 7 + serial - 3 * floor(serial / 3));
        }}

"#
            );
            let options = sessions(
                "renewal(1.5)",
                Overrides {
                    warmup: Some(0.0),
                    seed: Some(1),
                    ..common::horizon(120.0)
                },
            );
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

/// vLLM's procedure is spelt `running.preempted == 0` (#721); a gate that
/// computes the same written otherwise (`!`, `0 ==`, `< 1`) makes the
/// schedule a body, which runs as the procedure does. The examples preempt
/// nothing, so the gate is tested here, where it decides: two requests of
/// 40 tokens and 60 out on 8 blocks of 16 cannot both finish resident, so
/// requests are preempted, and with no gate a step may take back the request
/// it preempted, which makes the run another (scheduler.py:869). The claim
/// is that the reports are equal, not what they count.
#[test]
fn the_gates_spellings_run_as_the_procedure() {
    let run_with = |gate: &str| {
        let src = format!(
            r#"
        device gpu {{ kv cap 8 * 16; }}
        engine llm on gpu {{
          reqs cap 16;
          tokens cap 40;
          scheduler {{
            advance running;
            admit waiting {gate} taking reqs (1), kv (min(request.input, 40));
          }}
          worker {{ forward (1); }}
        }}
        pool kv on gpu {{ block 16; evict lru; preempt lifo; }}
        pool reqs on llm {{ }}
        server {{ llm (40, 60); }}

"#
        );
        run(
            &src,
            &sessions(
                "batch(2)",
                Overrides {
                    warmup: Some(0.0),
                    seed: Some(1),
                    ..common::horizon(2000.0)
                },
            ),
        )
    };
    let procedure = run_with("while (running.preempted == 0)");
    assert!(
        procedure.pool("kv").unwrap().preemptions > 0,
        "{}",
        procedure.text()
    );
    assert_ne!(run_with("").text(), procedure.text(), "the gate decides");
    for gate in [
        "while (!running.preempted)",
        "while (0 == running.preempted)",
        "while (running.preempted < 1)",
    ] {
        assert_eq!(run_with(gate).text(), procedure.text(), "{gate}");
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
    let file = "examples/papers/bari_rad.sq";
    let p = common::example(file, &common::horizon(20.0)).unwrap_or_else(|e| panic!("{file}: {e}"));
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
          scheduler { advance running; admit waiting while (batch.tokens > 0) taking reqs (1); }
          worker { forward (1); }
        }
        pool reqs on llm { }
        server { llm (2, 1);
        }

"#;
    let r = run(
        src,
        &sessions(
            "batch(3)",
            Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(20.0)
            },
        ),
    );
    assert!(r.stages[0].idle_with_work, "{}", r.text());
    assert!(r.text().contains("idle: engine `llm`"), "{}", r.text());
}

/// A guard is a test: a value other than 1 or 0 fails the run.
#[test]
fn a_guard_that_is_not_a_test_fails_the_run() {
    let src = r#"
        device gpu { }
        engine llm on gpu {
          reqs cap 8;
          tokens cap 8;
          scheduler { advance running; admit waiting while (running.count + 2) taking reqs (1); }
          worker { forward (1); }
        }
        pool reqs on llm { }
        server { llm (2, 1);
        }

"#;
    let (workload, ov) = sessions(
        "batch(1)",
        Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(20.0)
        },
    );
    let e = common::run(&common::main_source(src), &workload, &ov).unwrap_err();
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
        let items = taking(items, "reqs (1)");
        format!(
            r#"
        device gpu {{ }}
        engine llm on gpu {{
          reqs cap 64;
          tokens cap 64;
          {items}
          worker {{ forward (1); }}
        }}
        pool reqs on llm {{ }}
        server {{
          set t0 = now;
          llm (1, 31);
          observe wait = llm.admitted - t0;
        }}

"#
        )
    };
    let wait = |items: &str| {
        let r = run(
            &prog(items),
            &sessions(
                "renewal(1)",
                Overrides {
                    warmup: Some(0.0),
                    seed: Some(1),
                    ..common::horizon(40.0)
                },
            ),
        );
        r.observe("wait").unwrap().samples.clone()
    };
    let every =
        wait("scheduler { advance running; admit waiting while (running.preempted == 0); }");
    assert!(every.iter().all(|&w| w == 0.0), "{every:?}");
    let alternate = wait(
        "state just = 0; \
         scheduler { \
           advance running; if (just == 0) { admit waiting; } \
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
          scheduler {
            set n = n + 1;
            advance running only (decoding);
            admit waiting while (running.count == 0) taking reqs (1);
          }
          worker { forward (1); }
        }
        pool reqs on llm { }
        server { llm (100, 1);
        }
        gauge count = n;

"#;
    let r = run(
        src,
        &sessions(
            "renewal(1)",
            Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(30.0)
            },
        ),
    );
    // the first session is admitted and gets 4 tokens; after that its
    // prefill is excluded and nothing else may be admitted: every later
    // arrival is an event and a try, and none is an iteration
    assert_eq!(r.gauge("count").unwrap().max, 1.0, "{}", r.text());
}

#[test]
fn a_register_is_the_stage_s_own() {
    let prog = |items: &str, session: &str| {
        let items = taking(items, "reqs (1)");
        format!(
            r#"
        device gpu {{ }}
        device tpu {{ }}
        engine llm on gpu {{ reqs cap 8; tokens cap 8; {items} worker {{ forward (1); }} }}
        pool reqs on llm {{ }}
        engine other on tpu {{
          tokens cap 8;
          state r = 0;
          scheduler {{ advance running; admit waiting; }}
          worker {{ forward (1); }}
        }}
        server {{ llm (2, 1); other (1, 1); {session}
        }}

"#
        )
    };
    let err = |items: &str, session: &str| {
        compile(
            &common::main_source(&prog(items, session)),
            &sessions("batch(1)", common::horizon(20.0)),
        )
        .err()
        .unwrap_or_else(|| panic!("`{items}` `{session}` linked"))
    };
    let body = "state k = 0; scheduler { advance running; admit waiting; set k = k + 1; }";
    assert!(
        compile(
            &common::main_source(&prog(body, "")),
            &sessions("batch(1)", common::horizon(20.0))
        )
        .is_ok()
    );
    assert!(err("state k = 0; scheduler { advance running; admit waiting while (running.preempted == 0); }", "").contains("is set by nothing"));
    // an `only` is a body, but none that sets the register
    assert!(
        err(
            "state k = 0; scheduler { advance running only (decoding); \
             admit waiting only (decoding) while (running.preempted == 0); }",
            ""
        )
        .contains("is set by nothing")
    );
    assert!(
        err(
            "scheduler { advance running; admit waiting; set r = 1; }",
            ""
        )
        .contains("another engine")
    );
    assert!(err(body, "set x = k;").contains("register"));
    assert!(
        err(
            "state cached = 0; scheduler { advance running; admit waiting; }",
            ""
        )
        .contains("taken")
    );
    assert!(
        err(
            "state k = 0; scheduler { advance running; admit waiting; set k = now; }",
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
    // `b`'s request is admitted `taking` its pools (none if empty), and
    // the server is `server`; `other` is a capacity of `b`'s device, which
    // `b` does not admit
    let prog = |extra: &str, taking: &str, server: &str| {
        let taking = if taking.is_empty() {
            String::new()
        } else {
            format!(" taking {taking}")
        };
        format!(
            r#"
        device gpu {{ other cap 8; }}
        engine b on gpu {{
          reqs cap 8;
          tokens cap 8;
          state go = 0;
          scheduler {{ advance running; admit waiting{taking}; set go = 1; }}
          worker {{ forward (1); }}
        }}
        pool reqs on b {{ }}
        pool other on gpu {{ }}
        {extra}
        server {{ {server}
        }}

"#
        )
    };
    let ok = |extra: &str, taking: &str, server: &str| {
        compile(
            &common::main_source(&prog(extra, taking, server)),
            &sessions("batch(1)", common::horizon(20.0)),
        )
    };
    let call = "b (2, 1);";
    let base = "reqs (1)";
    // its pool's header and keys, a gauge
    assert!(ok("gauge g = go;", "reqs (1 + go)", call).is_ok());
    // a hold whose first pool, where it waits, the stage admits, whatever
    // else it holds (SGLang's `reqs` and `kv`); not one that waits elsewhere
    assert!(ok("", "reqs (1), other (1 + go)", call).is_ok());
    let e = ok("", "other (1), reqs (1 + go)", call).unwrap_err();
    assert!(e.contains("`go` is engine `b`'s register"), "{e}");
    // a ps capacity, another stage, a hold on a pool admitted at settle time
    for (extra, taking, server) in [
        ("stage p : ps(1 + go);", base, call),
        (
            "device tpu { }
        engine a on tpu {
          tokens cap 8;
          scheduler {
            advance running only (go == 1);
            admit waiting only (go == 1) while (running.preempted == 0);
          }
          worker { forward (1); }
        }",
            base,
            call,
        ),
        ("", "", "hold other (cost(other, 1 + go)) { b (2, 1); }"),
    ] {
        let e = ok(extra, taking, server)
            .err()
            .unwrap_or_else(|| panic!("{extra} {taking} {server} linked"));
        assert!(e.contains("`go` is engine `b`'s register"), "{e}");
    }
    let arr = "device tpu[2] { }
        engine c[2] on tpu {
          tokens cap 8;
          state q = 0;
          scheduler { advance running; admit waiting; }
          worker { forward (1); }
        }";
    assert!(ok(arr, base, call).unwrap_err().contains("engine family"));
    let given = "claim c given (go == 0): at end (1);";
    assert!(ok(given, base, call).is_err());
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
          scheduler {
            advance running only (decoding);
            set k = k + 1;
            admit waiting only (decoding) taking reqs (1);
          }
          worker { forward (1); }
        }
        pool reqs on llm { }
        server { llm (2, 1);
        }
        gauge seen = k;

"#;
    let r = run(
        src,
        &sessions(
            "batch(2)",
            Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(10.0)
            },
        ),
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
          scheduler {
            set r = 10;
            advance running;
            admit waiting taking reqs (1), kv (100) reserve (100 + r);
          }
          worker { forward (0.001 + batch.tokens * 1e-5); }
        }
        pool reqs on llm { }
        pool kv on gpu { evict lru; }
        server {
          llm (100, 10);
        }

"#;
    let r = run(
        src,
        &sessions(
            "renewal(2)",
            Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(50.0)
            },
        ),
    );
    assert_eq!(r.pool("kv").unwrap().rejected, 0, "{}", r.text());
    assert!(r.pool("reqs").unwrap().admissions > 20, "{}", r.text());
}

/// A schedule's `else if` is the `else` block holding one `if`.
#[test]
fn a_schedule_else_if_is_the_nested_if() {
    let compile = |schedule: &str| {
        let schedule = taking(schedule, "reqs (1)");
        let src = format!(
            r#"
        device gpu {{ }}
        engine llm on gpu {{
          reqs cap 8;
          tokens cap 8;
          scheduler {{ {schedule} }}
          worker {{ forward (1); }}
        }}
        pool reqs on llm {{ }}
        server {{ llm (2, 1);
        }}
"#
        );
        compile(
            &common::main_source(&src),
            &sessions("batch(1)", common::horizon(20.0)),
        )
        .map(|p| p.to_json())
    };
    let ir = |schedule: &str| compile(schedule).unwrap_or_else(|e| panic!("`{schedule}`: {e}"));
    // a `let` stands at the schedule's top, not in an `else if`'s block
    let e = compile(
        "advance running; if (running.count == 0) { admit waiting; } \
         else if (running.count < 4) { let x = 1; admit waiting; }",
    )
    .expect_err("a `let` in an `else if` links");
    assert!(e.contains("not in an `if`"), "{e}");
    // and the retired `branch` says what it is now
    let e = compile("advance running; branch (running.count == 0) { admit waiting; }")
        .expect_err("`branch` links");
    assert!(e.contains("is now `if (e)`"), "{e}");
    assert_eq!(
        ir("if (running.count == 0) { admit waiting; } \
            else if (running.count < 4) { advance running; admit waiting; } \
            else { advance running; }"),
        ir("if (running.count == 0) { admit waiting; } \
            else { if (running.count < 4) { advance running; admit waiting; } \
            else { advance running; } }"),
    );
}
