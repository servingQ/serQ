//! Deterministic checks of the pool semantics: admission with eviction,
//! eviction orders, block-level caches, spilling to a tier, growth with
//! and without preemption, priority queues.

mod common;

use serq::{Overrides, run_source};

fn run(src: &str, options: &Overrides) -> serq::Report {
    run_source(&common::main_source(src), options, None).unwrap()
}

/// A hold nested in another on the same pool leaves the outer hold's place
/// among the holders when it ends: `holders(kv)` read 0 between the two
/// releases, while the outer hold still held `kv`.
#[test]
fn an_inner_hold_leaves_the_outer_one_holding() {
    let src = "pool kv { cap 100; }
        stage svc : delay;
        workload { arrive batch(1); }
        server {
          hold kv (cost(kv, 10)) {
            hold kv (cost(kv, 10)) { run svc (cost(svc, 1)); }
            observe after_inner = holders(kv);
            run svc (cost(svc, 1));
          }
        }";
    let r = run(src, &common::horizon(10.0));
    assert_eq!(r.observe("after_inner").unwrap().samples, [1.0]);
}

/// Three sessions with contexts 10, 20, 30 on a pool of 55: the third
/// admission (30 with 30 cached) must evict one suspended session.
/// Shortest-first drops the 10-token one; LRU drops the one released
/// first (also 10 here); `by (-size)` (longest first) drops the 20-token
/// one.
#[test]
fn eviction_order_is_the_declared_key() {
    for (order, want_hit) in [
        ("evict by (waiting, size);", [1.0]),
        ("evict lru;", [1.0]),
        ("evict by (-size);", [0.0]),
    ] {
        let src = format!(
            r#"
        pool kv {{ cap 55; {order} }}
        stage svc : fifo;
        stage gate : delay;
        workload {{ arrive batch(3); init {{ set c = 10 * (serial + 1); }}
          session {{ turn;
            end;

          }}
        }}
        server {{
          run gate (cost(gate, serial));                 // 0, 1, 2: sequential first turns
          hold kv (cost(kv, c)) {{ run svc (cost(svc, 0.1)); }} cache (cost(kv, c));
          run gate (cost(gate, 10));
          branch (serial == 1) {{
            hold kv (cost(kv, c)) {{ observe hit = cached >= c; run svc (cost(svc, 0.1)); }} cache (cost(kv, c));
          }}
        }}

"#
        );
        let r = run(&src, &common::horizon(100.0));
        let hits = &r.observe("hit").unwrap().samples;
        assert_eq!(hits, &want_hit, "{order}\n{}", r.text());
    }
}

/// Sessions in a tool call are evicted before sessions waiting in a
/// queue (`waiting`), whatever their size: s0 (8 cached, queued for the
/// slot) survives and s1 (10 cached, in a tool call) is evicted when s2
/// needs 20 of a 30-token pool. Plain shortest-first would drop s0.
#[test]
fn queued_sessions_are_evicted_after_suspended_ones() {
    for (order, want0, want1) in [
        ("evict by (waiting, size);", 1.0, 0.0),
        ("evict by (size);", 0.0, 1.0),
    ] {
        let src = format!(
            r#"
        pool slot {{ cap 1; }}
        pool kv {{ cap 30; {order} }}
        stage svc : fifo;
        stage gate : delay;
        workload {{ arrive batch(4);
          session {{ turn;
            end;

          }}
        }}
        server {{
          branch (serial == 0) {{
            hold slot (cost(slot, 1)) {{ hold kv (cost(kv, 8)) {{ run svc (cost(svc, 1)); }} cache (cost(kv, 8)); }}
            run gate (cost(gate, 2));                                   // t = 3: queue for the slot
            hold slot (cost(slot, 1)) {{ hold kv (cost(kv, 8)) {{ observe hit0 = cached >= 8; run svc (cost(svc, 0.1)); }} cache (cost(kv, 0)); }}
          }}
          branch (serial == 1) {{
            run gate (cost(gate, 1));
            hold slot (cost(slot, 1)) {{ hold kv (cost(kv, 10)) {{ run svc (cost(svc, 1)); }} cache (cost(kv, 10)); }}
            run gate (cost(gate, 5));                                   // tool call t = 2..7
            hold slot (cost(slot, 1)) {{ hold kv (cost(kv, 10)) {{ observe hit1 = cached >= 10; run svc (cost(svc, 0.1)); }} cache (cost(kv, 0)); }}
          }}
          branch (serial == 2) {{
            run gate (cost(gate, 3.5));
            hold kv (cost(kv, 20)) {{ run svc (cost(svc, 0.1)); }}               // no slot needed: evicts at t = 3.5
          }}
          branch (serial == 3) {{
            run gate (cost(gate, 2));
            hold slot (cost(slot, 1)) {{ run gate (cost(gate, 2)); }}               // blocks the slot t = 2..4
          }}
        }}

"#
        );
        let r = run(&src, &common::horizon(100.0));
        assert_eq!(
            r.observe("hit0").unwrap().samples,
            vec![want0],
            "{order}\n{}",
            r.text()
        );
        assert_eq!(
            r.observe("hit1").unwrap().samples,
            vec![want1],
            "{order}\n{}",
            r.text()
        );
    }
}

/// A pool with `block b` rounds allocations up and caches down to blocks,
/// and evicts a block at a time from the tail of the least recently
/// released entry.
#[test]
fn block_pools_round_and_evict_by_block() {
    let src = r#"
        pool kv { cap 100; block 10; evict lru; }
        stage svc : fifo;
        stage gate : delay;
        workload { arrive batch(2);
          session { turn;
            end;

          }
        }
        server {
          run gate (cost(gate, serial));
          // s0 takes 55 -> 60 allocated, caches 55 -> 50 (five full blocks)
          // s1 takes 70 -> needs 70 of 100 - 0 used; cached 50 -> evict 2 blocks
          hold kv (cost(kv, serial == 0 ? 55 : 70)) { observe used = used(kv); run svc (cost(svc, 1)); } cache (cost(kv, serial == 0 ? 55 : 0));
          run gate (cost(gate, 10));
          branch (serial == 0) { hold kv (cost(kv, 55)) { observe cached0 = cached; run svc (cost(svc, 0.1)); } cache (cost(kv, 0)); }
        }

"#;
    let r = run(src, &common::horizon(100.0));
    assert_eq!(
        r.observe("used").unwrap().samples,
        vec![60.0, 70.0],
        "{}",
        r.text()
    );
    assert_eq!(r.observe("cached0").unwrap().samples, vec![30.0]);
    let kv = r.pool("kv").unwrap();
    assert_eq!(kv.evicted_units, 20.0);
    assert_eq!(kv.evicted_entries, 0);
}

/// `spill T via L (work) when (cond)`: an evicted prefix is written to a
/// tier over a link; the session fetches it back (`cachedin`).
#[test]
fn spill_to_a_tier_and_fetch_back() {
    let src = r#"
        pool kv { cap 30; evict lru; spill tier via link (size / 100) when (size >= 20); }
        pool tier { cap inf; }
        stage svc : fifo;
        stage link : fifo;
        stage gate : delay;
        workload { arrive batch(2); init { set c = serial == 0 ? 20 : 25; }
          session { turn;
            end;

          }
        }
        server {
          run gate (cost(gate, serial));
          hold kv (cost(kv, c)) { run svc (cost(svc, 1)); } cache (cost(kv, c));
          run gate (cost(gate, 5));
          branch (serial == 0) {
            observe in_tier = cachedin(tier);
            branch (cachedin(tier) > 0) { run link (cost(link, cachedin(tier) / 100)); observe fetched = 1; drop tier; }
            hold kv (cost(kv, c)) { run svc (cost(svc, 0.1)); }
          }
        }

"#;
    let r = run(src, &common::horizon(100.0));
    assert_eq!(
        r.observe("in_tier").unwrap().samples,
        vec![20.0],
        "{}",
        r.text()
    );
    assert_eq!(r.observe("fetched").unwrap().samples, vec![1.0]);
    // s0's 20 spilled when s1 admitted, s1's 25 when s0 re-admitted
    assert_eq!(r.pool("kv").unwrap().spills, 2);
    assert_eq!(r.stage("link").unwrap().completed, 3);
}

/// A spill predicate sees `waiting`, as the spec lists for it (the wait
/// channel: spill the prefix of a session that is waiting to come back).
/// s0 caches 6 and later queues for 20 behind s1's 24; s2, ahead of s0 by
/// priority, takes 6 and must evict s0's entry while s0 waits: with
/// `when (waiting)` that is the one spill, with `when (!waiting)` none.
#[test]
fn a_spill_predicate_sees_whether_the_session_is_queued() {
    let prog = |when: &str| {
        format!(
            "pool kv {{ cap 30; evict lru; queue by (pri); spill tier via link (1) when ({when}); }}
        pool tier {{ cap inf; }}
        stage svc : fifo(3);
        stage link : fifo;
        stage gate : delay;
        workload {{ arrive batch(3); init {{ set pri = serial == 2 ? 0 : 1; }}
          session {{ turn;
            end;

          }}
        }}
        server {{
          branch (serial == 0) {{
            hold kv (cost(kv, 6)) {{ run svc (cost(svc, 0.5)); }} cache (cost(kv, 6));
            run gate (cost(gate, 0.6));
            hold kv (cost(kv, 20)) {{ run svc (cost(svc, 0.1)); }}
          }}
          branch (serial == 1) {{ run gate (cost(gate, 0.5)); hold kv (cost(kv, 24)) {{ run svc (cost(svc, 10)); }} }}
          branch (serial == 2) {{ run gate (cost(gate, 1.2)); hold kv (cost(kv, 6)) {{ run svc (cost(svc, 1)); }} }}
        }}
        "
        )
    };
    let r = run(&prog("waiting"), &common::horizon(100.0));
    assert_eq!(r.pool("kv").unwrap().spills, 1, "{}", r.text());
    assert_eq!(r.pool("kv").unwrap().evicted_entries, 1, "{}", r.text());
    let r = run(&prog("!waiting"), &common::horizon(100.0));
    assert_eq!(r.pool("kv").unwrap().spills, 0, "{}", r.text());
    assert_eq!(r.pool("kv").unwrap().evicted_entries, 1, "{}", r.text());
}

/// `grow` with `preempt none` waits for room; the holder resumes when a
/// release makes room.
#[test]
fn grow_waits_under_preempt_none() {
    let src = r#"
        pool kv { cap 100; preempt none; }
        stage svc : fifo(2);
        stage gate : delay;
        workload { arrive batch(2);
          session { turn;
            end;

          }
        }
        server {
          run gate (cost(gate, serial));
          hold kv (cost(kv, 50)) {
            run svc (cost(svc, 5));
            branch (serial == 0) { set t = now; grow kv (cost(kv, 30)); observe waited = now - t; }
          }
          observe done = now;
        }

"#;
    let r = run(src, &common::horizon(100.0));
    // s0 asks for 30 more at t = 5 while s1 holds 50 until t = 6: waits 1
    assert_eq!(
        r.observe("waited").unwrap().samples,
        vec![1.0],
        "{}",
        r.text()
    );
    assert_eq!(r.pool("kv").unwrap().preemptions, 0);
}

/// #326: a hold preempted before it computes anything caches nothing of
/// what it was allocated. Session 1, admitted last, has run 2 of its 5
/// seconds and advanced no position when session 0's growth preempts it; it
/// used to come back with `cached = 10` (its 12 units, less what the
/// growth evicted) while `computed` read 0. A preempted hold caches its
/// position, which `computed` reads too; the scope's end still counts a
/// hold without a `growing` run as having computed what it holds.
#[test]
fn a_preempted_hold_caches_what_it_computed() {
    let src = r#"
        pool kv { cap 20; block 1; preempt lifo; }
        stage d : delay;
        workload { arrive batch(2);
          session { turn;
            end;

          }
        }
        server {
          branch (serial == 0) {
            hold kv (cost(kv, 5)) { run d (cost(d, 2)); grow kv (cost(kv, 5)); run d (cost(d, 10)); }
          } else {
            hold kv (cost(kv, 12)) {
              observe cached_at_admission = cached;
              observe computed_at_admission = computed;
              run d (cost(d, 5));
            } cache (cost(kv, 12));
          }
        }

"#;
    let r = run(
        src,
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(100.0)
        },
    );
    assert_eq!(r.pool("kv").unwrap().preemptions, 1, "{}", r.text());
    // the scope's end keeps its rule: session 1, done, caches what it holds
    // (the preemption cached nothing, so anything cached is from the end)
    assert!(r.pool("kv").unwrap().mean_cached > 0.0, "{}", r.text());
    for name in ["cached_at_admission", "computed_at_admission"] {
        assert_eq!(
            r.observe(name).unwrap().samples,
            vec![0.0, 0.0],
            "{name}: {}",
            r.text()
        );
    }
}

/// `queue by (key)`: a priority queue admits the smallest key first.
#[test]
fn priority_queue_orders_admissions() {
    let src = r#"
        pool kv { cap 10; queue by (prio); }
        stage svc : fifo;
        workload { arrive batch(3); init { set prio = 2 - serial; }
          session { turn;
            end;

          }
        }
        server {
          hold kv (cost(kv, 10)) { observe order = serial; run svc (cost(svc, 1)); }
        }

"#;
    let r = run(src, &common::horizon(100.0));
    assert_eq!(
        r.observe("order").unwrap().samples,
        vec![2.0, 1.0, 0.0],
        "{}",
        r.text()
    );
}

/// A hold that can never fit ends the session (vLLM refuses a prompt
/// longer than max_model_len before scheduling it, input_processor.py:512-536).
#[test]
fn oversized_requests_are_rejected() {
    let src = r#"
        pool kv { cap 10; }
        stage svc : fifo;
        workload { arrive batch(2);
          session { turn; end;
          }
        }
        server { hold kv (cost(kv, serial == 0 ? 20 : 5)) { run svc (cost(svc, 1)); } observe done = serial;
        }

"#;
    let r = run(src, &common::horizon(100.0));
    assert_eq!(r.observe("done").unwrap().samples, vec![1.0]);
    assert_eq!(r.pool("kv").unwrap().rejected, 1);
}

/// Admission waits for the reservation, not only the units, so a reservation
/// above the cap can never fit either: the session ends, and the sessions
/// queued behind it are admitted. Without the check the first session waits
/// at the head of the queue for ever and the other two with it.
#[test]
fn an_oversized_reservation_is_rejected() {
    let src = r#"
        pool kv { cap 10; }
        stage svc : fifo;
        workload { arrive batch(3);
          session { turn; end;
          }
        }
        server { hold kv (cost(kv, 1)) reserve (cost(kv, serial == 0 ? 20 : 1)) { run svc (cost(svc, 1)); } observe done = serial;
        }

"#;
    let r = run(src, &common::horizon(100.0));
    assert_eq!(r.observe("done").unwrap().samples, vec![1.0, 2.0]);
    assert_eq!(r.pool("kv").unwrap().rejected, 1);
}

/// A hold that can never fit: a constant one does not link, a computed one
/// ends its session and the report says so.
#[test]
fn a_hold_larger_than_the_cap_is_refused_or_reported() {
    // constant units that fit no member of the reference do not link
    // (#271), rounded to blocks, `reserve` included
    for hold in [
        "hold kv (cost(kv, 20))",
        "hold kv (cost(kv, 2 * 5 + 1))",
        "hold kv (cost(kv, 1)) reserve (cost(kv, 11))",
        "hold kv (cost(kv, max(20, 1)))",
        "hold kv (cost(kv, 1 > 0 ? 20 : 1))",
        "hold kv2[0] (cost(kv2, 20))",
        "hold kv2[serial] (cost(kv2, 20))",
    ] {
        let src = format!(
            "pool kv {{ cap 10; }} pool kv2[2] {{ cap 10; }} stage d : delay;
        workload {{ arrive batch(1);
          session {{ turn; end;
          }}
        }}
        server {{ {hold} {{ run d (cost(d, 1)); }}
        }}
        "
        );
        let e = run_source(&common::main_source(&src), &common::horizon(10.0), None).unwrap_err();
        let said = if hold.contains("serial") {
            "more than the cap of every member (`kv2`: 10)"
        } else {
            "more than its cap 10"
        };
        assert!(e.contains(said), "{hold}: {e}");
    }
    let e = run_source(
        &common::main_source(
            "pool kv { cap 10; block 4; } stage d : delay;
        workload { arrive batch(1);
          session { turn; end;
          }
        }
        server { hold kv (cost(kv, 9)) { run d (cost(d, 1)); }
        }
        ",
        ),
        &common::horizon(10.0),
        None,
    )
    .unwrap_err();
    assert!(
        e.contains("waits for 9 units (12 in blocks of 4), more than its cap 10"),
        "{e}"
    );
    // units the program computes are the run's: the session ends, counted
    // in `rej`, and the report says so
    let r = run(
        "pool kv { cap 10; } stage d : delay;
        workload { arrive batch(2); init { set u = 5 + 10 * serial; }
          session { turn; end;
          }
        }
        server { hold kv (cost(kv, u)) { run d (cost(d, 1)); }
        }
        ",
        &common::horizon(10.0),
    );
    assert_eq!(r.pool("kv").unwrap().rejected, 1, "{}", r.text());
    assert!(
        r.text()
            .contains("rej: 1 session(s) ended at pool `kv` asking for more than its cap"),
        "{}",
        r.text()
    );
}

/// Only a step stage's scheduler admits: `admit via` a FIFO, PS or delay
/// stage linked, and the pool's queue waited forever with nothing counted
/// stuck (#418, the program as found).
#[test]
fn admit_via_names_a_step_stage() {
    for (stage, kind) in [
        ("stage F : fifo;", "a fifo stage"),
        ("stage F : ps(2);", "a ps stage"),
        ("stage F : delay;", "a delay stage"),
    ] {
        let src = format!(
            "pool reqs {{ cap 4; admit via F; }}
  {stage}
  workload {{ arrive poisson(1); init {{ set t0 = now; }} }}
  server {{
    hold reqs (cost(reqs, 1)) {{ run F (cost(F, 1)); }}
    observe response = now - t0;
  }}"
        );
        let error =
            run_source(&common::main_source(&src), &common::horizon(100.0), None).unwrap_err();
        assert!(
            error.contains(&format!(
                "`F` is {kind}: only a step stage's scheduler admits"
            )),
            "{error}"
        );
    }
}

/// A hold that fits at admission but can never grow to what its body needs
/// preempts itself, re-enters at the head of the queue, and does it again:
/// a livelock the run would otherwise hide behind a preemption count. The
/// report counts the session once as `stuck` (preempted again at the same
/// position) and says so.
///
/// On 10 blocks of 16 (160 tokens) with a 1000-token budget: step 1
/// prefills the 100-token prompt (7 blocks), steps 2..61 decode tokens
/// 101..160 (block 8 at 113, 9 at 129, 10 at 145), step 62 needs an 11th
/// block, none is free, the request is `running[-1]` and preempts itself:
/// the step schedules nothing and is not skipped (vLLM's `schedule()` runs
/// it and admits nothing, scheduler.py:869). Step 63 re-admits and
/// prefills again. The cycle is 62 steps, so preemptions fall at 62, 124,
/// …, 372: six before the horizon of 400, the second of them at the same
/// position (160) as the first.
#[test]
fn a_hold_that_can_never_fit_is_reported_stuck() {
    let src = r#"
        device gpu { kv cap 160; }
        engine llm on gpu {
          reqs cap 4;
          tokens cap 1000;
          schedule { advance running; admit waiting while (running.preempted == 0); }
          execute (1);
        }
        pool reqs on llm { }
        pool kv on gpu { block 16; evict lru; preempt lifo; }
        workload { arrive batch(1);
          session { turn;
            end;

          }
        }
        server {
          hold reqs (cost(reqs, 1)), kv (cost(kv, 100)) reserve (cost(kv, 100)) {
            run llm prefill (cost(llm, 100)) growing kv;
            run llm decode (cost(llm, 100)) growing kv;
          }
        }

"#;
    let r = run(src, &common::horizon(400.0));
    let kv = r.pool("kv").unwrap();
    assert_eq!(kv.preemptions, 6, "{}", r.text());
    assert_eq!(kv.admissions, 7, "{}", r.text());
    assert_eq!(kv.stuck, 1, "{}", r.text());
    assert!(r.text().contains("stuck: 1 session(s)"), "{}", r.text());
    assert_eq!(r.ended, 0, "{}", r.text());
    // one step per unit of time, and the step that starts at the horizon
    // is counted when it starts
    assert_eq!(r.stage("llm").unwrap().iterations, 401, "{}", r.text());
}

/// The step that only preempted lasts `C` at zero tokens. With `cost tokens`
/// that is 0: the step ends at the same instant and the next one re-admits,
/// so a zero cost is one more event at that instant, not a loop. Prefill
/// 100 costs 100, sixty decodes cost 60, the preempting step 0: preemptions
/// at t = 160 and 320, the third prefill would end at 420 > 400.
#[test]
fn a_zero_cost_preempting_step_does_not_hang() {
    let src = r#"
        device gpu { kv cap 160; }
        engine llm on gpu {
          reqs cap 4;
          tokens cap 1000;
          schedule { advance running; admit waiting while (running.preempted == 0); }
          execute (batch.tokens);
        }
        pool reqs on llm { }
        pool kv on gpu { block 16; evict lru; preempt lifo; }
        workload { arrive batch(1);
          session { turn;
            end;

          }
        }
        server {
          hold reqs (cost(reqs, 1)), kv (cost(kv, 100)) reserve (cost(kv, 100)) {
            run llm prefill (cost(llm, 100)) growing kv;
            run llm decode (cost(llm, 100)) growing kv;
          }
        }

"#;
    let r = run(src, &common::horizon(400.0));
    let kv = r.pool("kv").unwrap();
    assert_eq!(kv.preemptions, 2, "{}", r.text());
    assert_eq!(kv.stuck, 1, "{}", r.text());
}

/// A `branch` guard is a test, 0 or 1. A computed fraction used to be drawn
/// as a probability without anyone asking for a draw; now it is an error
/// when evaluated (a constant one is a link error, `tests/lints.rs`).
const GUARD: &str = "
        stage svc : delay;
        workload { arrive batch(1); init { set c = 5; set K = 10; }
          session { turn; end;
          }
        }
        server { branch (GUARD) { run svc (cost(svc, 1)); }
        }
        ";

#[test]
fn a_computed_fraction_is_not_a_draw() {
    let error = run_source(
        &common::main_source(&GUARD.replace("GUARD", "c / K")),
        &common::horizon(100.0),
        None,
    )
    .unwrap_err();
    assert!(error.contains("the guard is 0.5, not 0 or 1"), "{error}");
}

#[test]
fn a_nan_guard_is_an_error() {
    let error = run_source(
        &common::main_source(&GUARD.replace("GUARD", "0 / 0")),
        &common::horizon(100.0),
        None,
    )
    .unwrap_err();
    assert!(error.contains("the guard is NaN, not 0 or 1"), "{error}");
}

#[test]
fn a_negative_guard_is_an_error() {
    let error = run_source(
        &common::main_source(&GUARD.replace("GUARD", "0 - 1")),
        &common::horizon(100.0),
        None,
    )
    .unwrap_err();
    assert!(error.contains("the guard is -1, not 0 or 1"), "{error}");
}

#[test]
fn a_boolean_guard_and_a_declared_draw_run() {
    run(&GUARD.replace("GUARD", "c < K"), &common::horizon(100.0));
    run(
        &GUARD.replace("branch (GUARD)", "branch with (c / K)"),
        &common::horizon(100.0),
    );
}

/// #230: a hold without a `cache` clause takes no part in the prefix
/// cache. One session sends the same 1000-token prompt every second; the
/// request's hold keeps `cache (1000)`, i.e. 992 (62 full blocks of 16).
/// Alone, turn 1 misses and every later turn hits: `cached` = min(992,
/// 992) > 0. Wrapped in an outer hold on the same pool, with or without a
/// `release kv;` first, the outer hold has no `cache` clause, so it leaves
/// the 992 cached units where they are and the inner hold finds them: the
/// same hits. (Before: the outer admission consumed the entry, so the inner
/// hold found none and every turn missed; the inner hold cached 992 again,
/// which the outer hold's end, without a clause, left alone.) With `cache (0)`
/// on the outer hold the program says the opposite, consume and keep
/// nothing, and every turn misses.
#[test]
fn a_hold_without_cache_leaves_the_prefix_to_the_hold_that_caches() {
    let request =
        "hold kv (cost(kv, min(cachedin(kv), 992) + 8)) at admission (hit = min(cachedin(kv), 992)) {
                     observe hit = cached > 0;
                     run llm prefill (cost(llm, 1000 - cached)) growing kv;
                   } cache (cost(kv, 1000));";
    for (wrap, want_hit) in [
        (request.to_string(), 1.0),
        (format!("hold kv (cost(kv, 0)) {{ {request} }}"), 1.0),
        (
            format!("hold kv (cost(kv, 0)) {{ release kv; {request} }}"),
            1.0,
        ),
        (
            format!("hold kv (cost(kv, 0)) {{ {request} }} cache (cost(kv, 0));"),
            0.0,
        ),
    ] {
        let src = format!(
            r#"
        device gpu {{ kv cap 100000; }}
        engine llm on gpu {{
          tokens cap 8192;
          schedule {{ advance running; admit waiting while (running.preempted == 0); }}
          execute (batch.tokens * 1e-5 + 1e-4);
        }}
        pool kv on gpu {{ block 16; evict lru; }}
        stage think : delay;
        workload {{ arrive closed(1);
          session {{ turn;
          }}
        }}
        server {{ loop {{ run think (cost(think, 1)); {wrap} }}
        }}

"#
        );
        let r = run(
            &src,
            &Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(20.0)
            },
        );
        let hits = &r.observe("hit").unwrap().samples;
        assert!(hits.len() >= 10, "{wrap}\n{}", r.text());
        assert_eq!(hits[0], 0.0, "the first turn is cold: {wrap}");
        assert!(
            hits[1..].iter().all(|&h| h == want_hit),
            "{wrap}: hits {hits:?}\n{}",
            r.text()
        );
    }
}

/// An amount a statement names is a number of units, tokens or seconds,
/// and an index names a member: NaN, a negative amount and an index that
/// is negative, fractional or past the array are program errors. They used
/// to be clamped to 0 in silence (#270).
#[test]
fn bad_amounts_and_indices_fail_the_run() {
    let fail = |stmt: &str| {
        let src = format!(
            "pool kv {{ cap 64; }} pool q[2] {{ cap 64; }} stage d : delay; stage a[2] : delay;
        workload {{ arrive batch(1); init {{ set z = 0; set i = 0; }}
          session {{ turn; end;
          }}
        }}
        server {{ {stmt}
        }}
        "
        );
        run_source(&common::main_source(&src), &common::horizon(10.0), None).unwrap_err()
    };
    for (stmt, said) in [
        (
            "run d (cost(d, z / z));",
            "`run d (cost(d, z / z))`: the amount is NaN",
        ),
        (
            "run d (cost(d, z - 5));",
            "`run d (cost(d, z - 5))`: the amount is -5",
        ),
        (
            "hold kv (cost(kv, z - 1)) { run d (cost(d, 1)); }",
            "`hold kv (cost(kv, z - 1))`: the amount is -1",
        ),
        (
            "hold kv (cost(kv, 8)) { grow kv (cost(kv, z - 1)); }",
            "`grow kv (cost(kv, z - 1))`: the amount is -1",
        ),
        (
            "hold kv (cost(kv, 8)) { load kv (cost(kv, z / z)); }",
            "`load kv (cost(kv, z / z))`: the amount is NaN",
        ),
        (
            "run a[i - 1] (cost(a, 1));",
            "index `i - 1` is -1: a member of an array of 2 is 0 to 1",
        ),
        ("run a[i + 0.5] (cost(a, 1));", "index `i + 0.5` is 0.5"),
        ("run a[i + 2] (cost(a, 1));", "index `i + 2` is 2"),
        (
            "hold q[i - 1] (cost(q, 1)) { run d (cost(d, 1)); }",
            "index `i - 1` is -1",
        ),
        (
            "hold kv (cost(kv, 1)) reserve (cost(kv, z / z)) { run d (cost(d, 1)); }",
            "`hold kv reserve (cost(kv, z / z))`: the amount is NaN",
        ),
        (
            "choose j in (z - 1) by (j);",
            "`choose … in (z - 1)`: the count is -1, not a whole number",
        ),
    ] {
        let e = fail(stmt);
        assert!(e.contains(said), "{stmt}: {e}");
    }
    // a constant one does not link
    for (stmt, said) in [
        ("run d (cost(d, -5));", "`run d (-5)`"),
        (
            "hold kv (cost(kv, 2 - 3)) { run d (cost(d, 1)); }",
            "`hold kv (-1)`",
        ),
        (
            "hold kv (cost(kv, 8)) { grow kv (cost(kv, -1)); }",
            "`grow kv (-1)`",
        ),
        (
            "hold kv (cost(kv, 8)) { load kv (cost(kv, 0 / 0)); }",
            "`load kv (NaN)`",
        ),
    ] {
        let src = format!(
            "pool kv {{ cap 64; }} stage d : delay;
        workload {{ arrive batch(1);
          session {{ turn; end;
          }}
        }}
        server {{ {stmt}
        }}
        "
        );
        let e =
            serq::compile_source(&common::main_source(&src), &common::horizon(10.0)).unwrap_err();
        assert!(
            e.contains(said) && e.contains("is a number, and not negative"),
            "{stmt}: {e}"
        );
    }
    // a constant index names a member, in IR as in text
    let mut p = serq::compile_source(
        &common::main_source(
            "stage a[2] : delay; workload { arrive batch(1);
          session { turn; end;
          }
        }
        server { run a[0] (cost(a, 1));
        } ",
        ),
        &common::horizon(10.0),
    )
    .unwrap();
    let serq::ir::CStmt::Run { stage, .. } = &mut p.blocks[p.session][1] else {
        panic!("the turn is followed by a run")
    };
    stage.index = Some(Box::new(serq::ir::CExpr::Num(-1.0)));
    let e = p.validate().unwrap_err();
    assert!(
        e.contains("stage index -1: a member of an array of 2 is 0 to 1"),
        "{e}"
    );
    // a decode is named as the kernel writes it (`run E decode (…)`)
    let e = run_source(
        &common::main_source(
            "device gpu { kv cap 64; }
        engine eng on gpu { tokens cap 8; schedule { advance running; admit waiting while (running.preempted == 0); } execute (1); }
        pool kv on gpu { }
        workload { arrive batch(1); init { set z = 0; }
          session { turn; end;
          }
        }
        server { hold kv (cost(kv, 8)) { run eng decode (cost(eng, z - 1)); }
        }
        ",
        ),
        &common::horizon(10.0),
        None,
    )
    .unwrap_err();
    assert!(
        e.contains("`run eng decode (cost(eng, z - 1))`: the amount is -1"),
        "{e}"
    );
    // zero is an amount: a run of no work, a hold of nothing
    let r = run(
        "pool kv { cap 64; } stage d : delay;
        workload { arrive batch(1); init { set z = 0; }
          session { turn; end;
          }
        }
        server { run d (cost(d, z)); hold kv (cost(kv, z)) { run d (cost(d, 1)); }
        }
        ",
        &common::horizon(10.0),
    );
    assert_eq!(r.ended, 1, "{}", r.text());
}

/// A hold takes each pool once. Twice, `grow` grew one entry, the pool's
/// `used` once, and the release gave both back: `used` went negative (#309).
#[test]
fn a_hold_takes_a_pool_once() {
    let src = "device gpu { kv cap 400; }
        engine llm on gpu {
          tokens cap 128;
          schedule { advance running each at most (128); admit waiting while (running.preempted == 0) each at most (128); }
          execute (0.001);
        }
        pool kv on gpu { block 16; }
        workload { arrive batch(3);
          session { turn;
            end;

          }
        }
        server {
          set prompt = 64;
          hold kv (cost(kv, 16)), kv (cost(kv, 16)) { run llm prefill (cost(llm, prompt)) growing kv; run llm decode (cost(llm, 40)) growing kv; }
        }
        ";
    let e = serq::compile_source(&common::main_source(src), &common::horizon(1.0)).unwrap_err();
    assert!(e.contains("a hold takes `kv` twice"), "{e}");
    // indices the run sets to one member: only the run can tell
    let e = run_source(
        &common::main_source(
            "pool kv[2] { cap 64; } stage d : delay;
        workload { arrive batch(1); init { set i = 1; set j = 1; }
          session { turn; end;
          }
        }
        server { hold kv[i] (cost(kv, 1)), kv[j] (cost(kv, 1)) { run d (cost(d, 1)); }
        }
        ",
        ),
        &common::horizon(1.0),
        None,
    )
    .unwrap_err();
    assert!(
        e.contains("`kv[i]` and `kv[j]` name the same member, `kv[1]`"),
        "{e}"
    );
}

/// A reserve that reads the deployment's state asks for more under load
/// and less later; joining the queue under load does not reject it (#364:
/// SGLang's admission test grows with the running requests, and a request
/// waits). Three sessions on a pool of 10 reserving `4 + 4·holders`: the
/// third joins with two holders (12 > 10) and is admitted when one leaves.
/// One that reads no state and asks for more than the cap is rejected as
/// before; one that reads state and never fits is named when the run ends.
#[test]
fn a_reserve_that_reads_the_state_waits_instead_of_being_rejected() {
    let prog = |reserve: &str| {
        format!(
            r#"
        pool reqs {{ cap 5; }}
        pool kv {{ cap 10; }}
        stage svc : delay;
        workload {{ arrive batch(3);
          session {{ turn;
            end;

          }}
        }}
        server {{
          hold reqs (cost(reqs, 1)), kv (cost(kv, 2)) reserve (cost(kv, {reserve})) {{ run svc (cost(svc, 1 + serial)); }}
          observe done = serial;
        }}

"#
        )
    };
    let r = run(
        &prog("4 + 4 * holders(reqs)"),
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(50.0)
        },
    );
    assert_eq!(
        r.observe("done").unwrap().samples,
        vec![0.0, 1.0, 2.0],
        "{}",
        r.text()
    );
    assert_eq!(r.pool("kv").unwrap().rejected, 0);
    // the third waited over the cap and was admitted: nothing is over at the end
    assert!(r.pools.iter().all(|q| q.over_cap.is_none()), "{}", r.text());
    let r = run(
        &prog("9 + serial"),
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(50.0)
        },
    );
    assert_eq!(r.pool("kv").unwrap().rejected, 1, "{}", r.text());
    // the head waits in `reqs`'s queue (the hold's first pool) and asks `kv`,
    // whose cap is 10, for 11: the note names the pool asked
    let r = run(
        &prog("11 + 0 * holders(reqs)"),
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(50.0)
        },
    );
    assert_eq!(
        r.pool("kv").unwrap().over_cap,
        Some(("reqs".to_string(), 11.0)),
        "{}",
        r.text()
    );
    assert!(r.pool("reqs").unwrap().over_cap.is_none());
    assert!(
        r.text()
            .contains("over: the head of pool `reqs`'s queue asks `kv` for 11"),
        "{}",
        r.text()
    );
}

/// TensorRT-LLM's GUARANTEED_NO_EVICT admits a request only if the blocks
/// every running request may still need are left
/// (`capacityScheduler.cpp` L265-L305 at bf414e37): `reserve held`. Three
/// requests reserve prompt + max_tokens = 10 and allocate the prompt, 4, on
/// a pool of 20, then grow by 6. Tested once, all three are admitted
/// (4 + 4 + 10 ≤ 20) and their growth preempts; held, the third waits for
/// the first to finish and nothing is preempted.
#[test]
fn a_held_reservation_counts_against_later_admissions() {
    let prog = |held: &str| {
        format!(
            r#"
        device gpu {{ kv cap 20; }}
        engine llm on gpu {{
          reqs cap 8;
          tokens cap 64;
          schedule {{ advance running; admit waiting while (running.preempted == 0); }}
          execute (1);
        }}
        pool reqs on llm {{ }}
        pool kv on gpu {{ preempt lifo; {held} }}
        workload {{ arrive batch(3);
          session {{ turn;
            end;

          }}
        }}
        server {{
          hold reqs (cost(reqs, 1)), kv (cost(kv, 4)) reserve (cost(kv, 10)) {{
            observe admitted = now;
            run llm prefill (cost(llm, 4)) growing kv;
            run llm decode (cost(llm, 6)) growing kv;
          }} cache (cost(reqs, kv, 0));
        }}

"#
        )
    };
    let once = run(
        &prog(""),
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(100.0)
        },
    );
    assert!(once.pool("kv").unwrap().preemptions > 0, "{}", once.text());
    let held = run(
        &prog("reserve held;"),
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(100.0)
        },
    );
    assert_eq!(held.pool("kv").unwrap().preemptions, 0, "{}", held.text());
    let admitted = &held.observe("admitted").unwrap().samples;
    // the first two at 0; the third when the first leaves, after its prefill
    // (1 iteration) and 6 decodes
    assert_eq!(admitted, &[0.0, 0.0, 7.0], "{}", held.text());
}

/// Each live hold's reservation counts once, for as long as that hold
/// lasts (the review of #371: counted by the pool's holders, a nested hold
/// was counted twice, and the outer one lost when the inner one ended).
/// Serial 0 holds 1 reserving 5, and inside it 1 reserving 5 again: 2
/// allocated and 8 reserved on a pool of 12.
#[test]
fn a_held_reservation_is_each_live_hold_s() {
    let prog = |after_inner: &str, second: u32| {
        format!(
            r#"
        pool kv {{ cap 12; reserve held; }}
        stage gate : delay;
        workload {{ arrive batch(2);
          session {{ turn;
            end;

          }}
        }}
        server {{
          run gate (cost(gate, serial));
          branch (serial == 0) {{
            hold kv (cost(kv, 1)) reserve (cost(kv, 5)) {{
              hold kv (cost(kv, 1)) reserve (cost(kv, 5)) {{ run gate (cost(gate, 10)); }}
              {after_inner}
            }}
          }}
          branch (serial == 1) {{
            hold kv (cost(kv, {second})) {{ observe admitted = now; run gate (cost(gate, 1)); }}
          }}
        }}

"#
        )
    };
    let admitted = |src: &str| {
        let r = run(
            src,
            &Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(100.0)
            },
        );
        r.observe("admitted").unwrap().samples.clone()
    };
    // 2 + 8 + 2 = 12: admitted on arrival, at 1
    assert_eq!(admitted(&prog("", 2)), [1.0]);
    // serial 0 starts at 0: the inner hold ends at 10, and the outer one
    // still reserves 4 until 20 (counted by holders, it was lost at 10):
    // 1 + 4 + 8 = 13 > 12 until then
    assert_eq!(admitted(&prog("run gate (cost(gate, 10));", 8)), [20.0]);
}
