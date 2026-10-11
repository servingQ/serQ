//! Ascend-style FCFS lanes and aging, selected on the admission clock.
//! Expected orders below follow the tagged queue's immediate/aged-long/short/
//! long precedence, not observed interpreter output.

mod common;
use serq::ir::{CExpr, CtxVar, DistKind};
use serq::{Program, compile_source, run_ir, run_source};

fn aging_source(bound: bool, key: &str) -> String {
    let (stage, pool) = if bound {
        (
            "device gpu { }
        engine llm on gpu {
          reqs cap 1;
          tokens cap 2;
          schedule { advance running; admit waiting while (running.preempted == 0); }
          execute (1);
        }",
            "pool reqs on llm",
        )
    } else {
        ("stage llm : fifo;", "pool reqs")
    };
    let cap = if bound { "" } else { "cap 1; " };
    let run = if bound { "run llm decode" } else { "run llm" };
    format!(
        r#"
        {stage}
        {pool} {{ {cap}queue by ({key}); }}
        stage delay : delay;
        workload {{ arrive batch(6);
          session {{ turn;
            end;

          }}
        }}
        server {{
          set prompt = serial == 1 || serial == 5 ? 256 : 64;
          set immediate = serial == 4;
          run delay (cost(delay, serial == 2 ? 1 : serial == 3 ? 2 : serial >= 4 ? 3 : 0));
          set queued = now;
          hold reqs (cost(reqs, 1)) {{
            observe selected = serial;
            observe admitted = now;
            {run} (cost(llm, serial == 0 ? 4 : 1));
          }}
        }}

"#
    )
}

#[test]
fn immediate_then_aged_long_then_short_at_both_admission_paths() {
    // Session 0 holds the slot until 4. Long 1 queues at 0; shorts 2,3
    // at 1,2; immediate 4 and long 5 at 3. At release: immediate wins,
    // then long 1 has crossed the 3-second threshold; shorts keep FIFO.
    // Long 5 cannot jump the shorts at 4 (wait=1), but by the selection
    // at 6 it reaches the threshold and must now precede both shorts.
    let key = "immediate ? 0 : prompt > 128 && waited >= 3 ? 1 : prompt <= 128 ? 2 : 3, serial";
    for bound in [false, true] {
        let src = aging_source(bound, key);
        let r = run_source(&common::main_source(&src), &common::horizon(20.0), None).unwrap();
        assert_eq!(r.ended, 6);
        assert_eq!(
            r.observe("selected").unwrap().samples,
            [0., 4., 1., 5., 2., 3.]
        );
        assert_eq!(
            r.observe("admitted").unwrap().samples,
            [0., 4., 5., 6., 7., 8.]
        );
        let p = compile_source(&common::main_source(&src), &common::horizon(20.0)).unwrap();
        let q = Program::from_json(&p.to_json()).unwrap();
        assert_eq!(r.text(), run_ir(&q, None).unwrap().text());
    }
}

#[test]
fn existing_single_key_is_recomputed_without_another_enqueue() {
    // The same scalar expression compiled in v8, but its key was cached
    // at enqueue (now=queued), so it gave 0,4,2,3,1,5. No new enqueue occurs
    // between 3 and the release at 4. Selection at 5 must still age long 1.
    let src = aging_source(
        false,
        "immediate ? 0 : prompt > 128 && now - queued >= 3 ? 1 : prompt <= 128 ? 2 : 3",
    );
    let r = run_source(&common::main_source(&src), &common::horizon(20.0), None).unwrap();
    assert_eq!(
        r.observe("selected").unwrap().samples,
        [0., 4., 1., 5., 2., 3.]
    );
}

#[test]
fn each_selection_reads_the_current_remaining_iteration_budget() {
    // At 0 only blocker 0 runs. Others queue during its one-second step.
    // At 1 budget=5: ascending serial selects 1:p2. Remaining budget=3:
    // descending serial selects 3:p3. Budget is exhausted, so 2:p1 enters
    // at 2. Evaluating once per iteration would incorrectly select 2 next.
    let src = r#"
        device gpu { }
        engine llm on gpu {
          reqs cap 4;
          tokens cap 5;
          schedule { advance running; admit waiting while (running.preempted == 0); }
          execute (1);
        }
        pool reqs on llm { queue by (budget_left(llm) >= 4 ? serial : -serial); }
        stage delay : delay;
        workload { arrive batch(4);
          session { turn;
            end;

          }
        }
        server {
          run delay (cost(delay, serial == 0 ? 0 : 0.25));
          hold reqs (cost(reqs, 1)) {
            observe selected = serial;
            observe admitted = now;
            branch (serial == 0) { run llm decode (cost(llm, 1)); }
            else { run llm prefill (cost(llm, serial == 2 ? 1 : serial == 1 ? 2 : 3)); }
          }
        }

"#;
    let r = run_source(&common::main_source(src), &common::horizon(10.0), None).unwrap();
    assert_eq!(r.ended, 4);
    assert_eq!(r.observe("selected").unwrap().samples, [0., 1., 3., 2.]);
    assert_eq!(r.observe("admitted").unwrap().samples, [0., 1., 1., 2.]);
}

#[test]
fn lexicographic_keys_preserve_enqueue_order_for_equal_keys() {
    // Blocker leaves at 1. Sessions 1,2,3 have equal first key; the
    // second key chooses 2 before 1,3. Equal keys 1,3 preserve FIFO.
    let src = r#"
        pool reqs { cap 1; queue by (0, serial == 2 ? 0 : 1); }
        stage engine : fifo;
        stage delay : delay;
        workload { arrive batch(4);
          session { turn;
            end;

          }
        }
        server {
          run delay (cost(delay, serial == 0 ? 0 : 0.25));
          hold reqs (cost(reqs, 1)) { observe selected = serial; run engine (cost(engine, 1)); }
        }

"#;
    let r = run_source(&common::main_source(src), &common::horizon(10.0), None).unwrap();
    assert_eq!(r.observe("selected").unwrap().samples, [0., 2., 1., 3.]);
}

#[test]
fn selection_rejects_random_hidden_and_wrong_moment_keys_in_source_and_ir() {
    for (key, message) in [
        ("~exp(1)", "may not draw"),
        ("tokens", "exists only"),
        ("secret", "hidden"),
    ] {
        let src = format!(
            "pool reqs {{ cap 1; queue by ({key}); }} workload {{ hidden secret; init {{ set secret = 1; }} \n  session {{ turn; end; \n  }}\n}} server {{\n}} "
        );
        let error = compile_source(&common::main_source(&src), &common::horizon(1.0))
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{key}: {error}");
    }
    let src = "pool reqs { cap 1; queue by (waited); } workload { session { turn; end; \n} }\nserver {\n} ";
    let p = compile_source(&common::main_source(src), &common::horizon(1.0)).unwrap();
    let mut bad = p.clone();
    bad.pools[0].queue = Some(vec![]);
    assert!(
        Program::from_json(&bad.to_json())
            .unwrap_err()
            .contains("at least one key")
    );
    let mut bad = p.clone();
    bad.version = 8;
    assert!(
        Program::from_json(&bad.to_json())
            .unwrap_err()
            .contains("version")
    );
    let mut bad = p.clone();
    bad.pools[0].queue = Some(vec![CExpr::Sample(DistKind::Exp, vec![CExpr::Num(1.)])]);
    assert!(
        Program::from_json(&bad.to_json())
            .unwrap_err()
            .contains("may not draw")
    );
    let mut bad = p;
    bad.pools[0].queue = Some(vec![CExpr::Ctx(CtxVar::Ntok)]);
    assert!(
        Program::from_json(&bad.to_json())
            .unwrap_err()
            .contains("exists only")
    );
    assert!(
        compile_source(
            &common::main_source(
                "workload { session { turn; end; \n} }\nserver { observe x = waited;\n} "
            ),
            &common::horizon(1.0)
        )
        .unwrap_err()
        .to_string()
        .contains("exists only")
    );
}

#[test]
fn a_selected_request_that_cannot_fit_still_blocks_lower_priority_requests() {
    // Blocker holds 3/4 until 4. Both waiters queue at .25; the preferred
    // long needs 2 and cannot fit, while the short needs only 1. Selection
    // does not turn into a fit-first scan: both wait until release at 4.
    let src = r#"
        pool kv { cap 4; queue by (serial == 1 && waited >= 0.5 ? 0 : serial); }
        stage delay : delay;
        stage service : delay;
        workload { arrive batch(3);
          session { turn;
            end;

          }
        }
        server {
          run delay (cost(delay, serial == 0 ? 0 : 0.25));
          hold kv (cost(kv, serial == 0 ? 3 : serial == 1 ? 2 : 1)) {
            observe selected = serial;
            observe admitted = now;
            run service (cost(service, serial == 0 ? 4 : 1));
          }
        }

"#;
    let r = run_source(&common::main_source(src), &common::horizon(10.0), None).unwrap();
    assert_eq!(r.observe("selected").unwrap().samples, [0., 1., 2.]);
    assert_eq!(r.observe("admitted").unwrap().samples, [0., 4., 4.]);
}

#[test]
fn resumed_holds_keep_prepend_priority_over_recomputed_keys() {
    // With six KV units and two prompt-2/output-3 requests, serial 1
    // prefills first (key -serial), 0 follows at 1. At 2 both decode;
    // at 3 the first resident's growth preempts the later resident 0.
    // New request 2 queues at 3.25. At 5 request 0 is restored before 2,
    // despite 2's smaller key. After its two prefill chunks, 2 enters at 7.
    let src = r#"
        device gpu { kv cap 6; }
        engine llm on gpu {
          reqs cap 2;
          tokens cap 4;
          schedule {
            exclusive prefill each at most (2);
            admit waiting while (running.preempted == 0) each at most (2);
          }
          execute (1);
        }
        pool reqs on llm { queue by (-serial); }
        pool kv on gpu { preempt lifo; }
        stage delay : delay;
        workload { arrive batch(3);
          session { turn;
            end;

          }
        }
        server {
          run delay (cost(delay, serial == 2 ? 3.25 : 0));
          hold reqs (cost(reqs, 1)), kv (cost(kv, min(known, left))) reserve (cost(kv, known))
          at admission (known = serial == 2 ? 0 : max(2, computed), left = budget_left(llm)) {
            observe selected = serial;
            observe admitted = now;
            branch (serial == 2) { run llm decode (cost(llm, 1)); }
            else {
              run llm prefill (cost(llm, known)) growing kv;
              run llm decode (cost(llm, 3 - (known - 2))) growing kv;
            }
          }
        }

"#;
    let r = run_source(&common::main_source(src), &common::horizon(20.0), None).unwrap();
    assert_eq!(r.ended, 3);
    assert_eq!(r.pool("kv").unwrap().preemptions, 1);
    assert_eq!(r.observe("selected").unwrap().samples, [1., 0., 0., 2.]);
    assert_eq!(r.observe("admitted").unwrap().samples, [0., 1., 5., 7.]);
}

#[test]
fn the_program_can_disable_aging_and_keep_short_request_precedence() {
    // Same arrivals and blocker, without the promotion rule: both shorts
    // precede both longs even after the longs have waited three seconds.
    for bound in [false, true] {
        let src = aging_source(bound, "immediate ? 0 : prompt <= 128 ? 1 : 2");
        let r = run_source(&common::main_source(&src), &common::horizon(20.0), None).unwrap();
        assert_eq!(
            r.observe("selected").unwrap().samples,
            [0., 4., 2., 3., 1., 5.]
        );
    }
}
