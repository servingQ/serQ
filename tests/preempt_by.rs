//! `preempt by (keys) [requeue head | requeue tail]`: the victim of a
//! growth that does not fit is the candidate with the least keys, and it
//! goes back to the head of its queue or to the tail (#356). `preempt lifo`
//! is `preempt by (-admission)`.

mod common;

use serq::{Overrides, compile_source, run_source};

fn run(src: &str, options: &Overrides) -> serq::Report {
    run_source(&common::main_source(src), options, None).unwrap()
}

/// Three requests decode on a pool that cannot hold them all. After six
/// decode steps every one has six outputs, so SGLang's order (fewest
/// outputs, then longest prompt) retracts serial 1, whose prompt is the
/// longest; `lifo` preempts serial 2, admitted last.
fn three(preempt: &str) -> String {
    format!(
        r#"
        device gpu {{ kv cap 40; }}
        engine llm on gpu {{
          reqs cap 3;
          tokens cap 64;
          schedule {{ advance running; admit waiting while (running.preempted == 0); }}
          execute (1);
        }}
        pool reqs on llm {{ }}
        pool kv on gpu {{ {preempt} }}
        workload {{ arrive batch(3);
          session {{ turn;
            end;

          }}
        }}
        server {{
          set prompt = serial == 0 ? 4 : serial == 1 ? 10 : 6;
          hold reqs (cost(reqs, 1)), kv (cost(kv, prompt)) {{
            branch (computed > 0) {{ observe victim = serial; }}
            run llm prefill (cost(llm, prompt)) growing kv;
            run llm decode (cost(llm, 12)) growing kv;
          }} cache (cost(reqs, kv, 0));
        }}

"#
    )
}

#[test]
fn the_victim_is_the_least_key() {
    for (preempt, first) in [
        ("preempt by (position - prompt, -prompt);", 1.0),
        ("preempt lifo;", 2.0),
        ("preempt by (-admission);", 2.0),
        // the earliest admitted: the opposite of lifo
        ("preempt by (admission);", 0.0),
    ] {
        let r = run(&three(preempt), &common::horizon(200.0));
        let victims = &r.observe("victim").unwrap().samples;
        assert_eq!(victims[0], first, "{preempt}: {victims:?}\n{}", r.text());
    }
}

/// `preempt lifo` is sugar: the same IR as `preempt by (-admission)`.
#[test]
fn lifo_is_by_minus_admission() {
    let ir = |p: &str| {
        let prog =
            compile_source(&common::main_source(&three(p)), &common::horizon(200.0)).unwrap();
        serde_json::to_string(&prog.pools).unwrap()
    };
    assert_eq!(ir("preempt lifo;"), ir("preempt by (-admission);"));
    assert_eq!(
        ir("preempt by (-admission) requeue head;"),
        ir("preempt lifo;")
    );
    assert_ne!(
        ir("preempt by (-admission) requeue tail;"),
        ir("preempt lifo;")
    );
}

/// Two requests run and a third waits for a slot. When the pool runs out,
/// serial 1 (admitted last) is preempted; at the head it is admitted again
/// before serial 2, at the tail after it.
#[test]
fn a_victim_requeues_at_the_head_or_the_tail() {
    for (requeue, order) in [
        ("requeue head", [0.0, 1.0, 1.0, 2.0]),
        ("requeue tail", [0.0, 1.0, 2.0, 1.0]),
    ] {
        let src = format!(
            r#"
        device gpu {{ kv cap 24; }}
        engine llm on gpu {{
          reqs cap 2;
          tokens cap 64;
          schedule {{ advance running; admit waiting while (running.preempted == 0); }}
          execute (1);
        }}
        pool reqs on llm {{ }}
        pool kv on gpu {{ preempt by (-admission) {requeue}; }}
        workload {{ arrive batch(3);
          session {{ turn;
            end;

          }}
        }}
        server {{
          hold reqs (cost(reqs, 1)), kv (cost(kv, 4)) {{
            observe admitted = serial;
            run llm prefill (cost(llm, 4)) growing kv;
            run llm decode (cost(llm, 10)) growing kv;
          }} cache (cost(reqs, kv, 0));
        }}

"#
        );
        let r = run(
            &src,
            &Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(200.0)
            },
        );
        let admitted = &r.observe("admitted").unwrap().samples;
        assert_eq!(
            admitted[..4],
            order,
            "{requeue}: {admitted:?}\n{}",
            r.text()
        );
    }
}

#[test]
fn a_preempt_key_reads_the_candidate_and_nothing_it_cannot_see() {
    let base = |preempt: &str, hidden: &str| {
        format!(
            r#"
        device gpu {{ kv cap 40; }}
        engine llm on gpu {{
          reqs cap 3;
          tokens cap 64;
          schedule {{ advance running; admit waiting while (running.preempted == 0); }}
          execute (1);
        }}
        pool reqs on llm {{ }}
        pool kv on gpu {{ {preempt} }}
        workload {{ arrive batch(3); {hidden}
          session {{ turn;
            end;

          }}
        }}
        server {{
          set prompt = 4;
          set o = 12;
          hold reqs (cost(reqs, 1)), kv (cost(kv, prompt)) {{
            run llm prefill (cost(llm, prompt)) growing kv;
            run llm decode (cost(llm, o)) growing kv;
          }} cache (cost(reqs, kv, 0));
        }}

"#
        )
    };
    let err = |preempt: &str, hidden: &str| {
        compile_source(
            &common::main_source(&base(preempt, hidden)),
            &Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(200.0)
            },
        )
        .err()
        .unwrap_or_else(|| panic!("`{preempt}` linked"))
    };
    assert!(err("preempt by (~exp(1));", "").contains("may not draw"));
    assert!(err("preempt by (budget_left(llm));", "").contains("budget_left"));
    assert!(err("preempt by (o);", "hidden o;").contains("hidden"));
    assert!(err("preempt by (remaining);", "").contains("remaining"));
    // `position` is a preempt key's alone
    let src = base("preempt lifo;", "").replace("set o = 12;", "set o = position;");
    assert!(
        compile_source(
            &common::main_source(&src),
            &Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(200.0)
            }
        )
        .unwrap_err()
        .contains("position")
    );
}

/// On a pool that is no engine's memory the candidates are its holders in
/// the order the pool admitted them, and `admission` is the place in that
/// order: `lifo` and `by (-admission)` with any further key pick the same
/// victim, though serial 0's latest admission is to another pool (B). The
/// reviewer's case of #360, where the two picked different victims.
#[test]
fn lifo_is_by_minus_admission_on_a_pool_no_engine_reads() {
    let prog = |preempt: &str| {
        format!(
            r#"
        pool a {{ cap 10; {preempt} }}
        pool b {{ cap 10; }}
        stage svc : delay;
        workload {{ arrive batch(2);
          session {{ turn;
            end;

          }}
        }}
        server {{
          branch (serial == 0) {{
            hold a (cost(a, 4)) {{
              run svc (cost(svc, 1));
              hold b (cost(b, 1)) {{ run svc (cost(svc, 1)); grow a (cost(a, 4)); run svc (cost(svc, 1)); }}
            }}
          }} else {{
            run svc (cost(svc, 0.5));
            hold a (cost(a, 4)) {{ observe s1 = computed; run svc (cost(svc, 5)); }}
          }}
        }}

"#
        )
    };
    let report = |p: &str| {
        let r = run(
            &prog(p),
            &Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(50.0)
            },
        );
        let pool = r.pool("a").unwrap();
        (
            pool.preemptions,
            pool.stuck,
            r.observe("s1").unwrap().samples.clone(),
        )
    };
    let lifo = report("preempt lifo;");
    // serial 1, the pool's last, is the victim; read by the session's
    // latest admission it would be serial 0, which preempts itself for
    // ever (`stuck`)
    assert!(lifo.0 > 0 && lifo.1 == 0, "{lifo:?}");
    assert_eq!(report("preempt by (-admission, 0);"), lifo);
    assert_eq!(report("preempt by (-admission) requeue head;"), lifo);
}

/// `requeue tail` puts the victim back as a newcomer: under `queue by`
/// its keys place it, ahead of a waiting session with a larger key.
#[test]
fn a_tail_victim_is_ordered_by_the_queue_keys() {
    let src = r#"
        device gpu { kv cap 24; }
        engine llm on gpu {
          reqs cap 2;
          tokens cap 64;
          schedule { advance running; admit waiting while (running.preempted == 0); }
          execute (1);
        }
        pool reqs on llm { queue by (rank); }
        pool kv on gpu { preempt by (-admission) requeue tail; }
        workload { arrive batch(3);
          session { turn;
            end;

          }
        }
        server {
          set rank = serial == 2 ? 9 : serial;
          hold reqs (cost(reqs, 1)), kv (cost(kv, 4)) {
            observe admitted = serial;
            run llm prefill (cost(llm, 4)) growing kv;
            run llm decode (cost(llm, 10)) growing kv;
          } cache (cost(reqs, kv, 0));
        }

"#;
    let r = run(
        src,
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(200.0)
        },
    );
    let admitted = &r.observe("admitted").unwrap().samples;
    // serial 1 (rank 1) is the victim; at the tail of a keyed queue its key
    // puts it ahead of serial 2 (rank 9)
    assert_eq!(
        admitted[..4],
        [0.0, 1.0, 1.0, 2.0],
        "{admitted:?}\n{}",
        r.text()
    );
}

/// A prefilling candidate reads `decoding` as 0: SGLang retracts from the
/// decode batch, so its order puts the decodes first.
#[test]
fn a_preempt_key_reads_decoding_and_not_computed() {
    let base = three("preempt by (1 - decoding, position - prompt, -prompt);");
    assert!(compile_source(&common::main_source(&base), &common::horizon(200.0)).is_ok());
    let err = compile_source(
        &common::main_source(&three("preempt by (computed);")),
        &common::horizon(200.0),
    )
    .unwrap_err();
    assert!(err.contains("`position`"), "{err}");
}
