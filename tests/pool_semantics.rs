//! Deterministic checks of the pool semantics: admission with eviction,
//! eviction orders, block-level caches, spilling to a tier, growth with
//! and without preemption, priority queues.

use serq::{Overrides, run_source};

fn run(src: &str) -> serq::Report {
    run_source(src, &Overrides::default(), None).unwrap()
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
            workload {{ arrive batch(3); init {{ set c = 10 * (serial + 1); }} }}
            session {{
              run gate (serial);                 // 0, 1, 2: sequential first turns
              hold kv (c) {{ run svc (0.1); }} cache (c);
              run gate (10);
              branch (serial == 1) {{
                hold kv (c) {{ observe hit = cached >= c; run svc (0.1); }} cache (c);
              }}
              end;
            }}
            run {{ horizon 100; }}
            "#
        );
        let r = run(&src);
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
            workload {{ arrive batch(4); }}
            session {{
              branch (serial == 0) {{
                hold slot (1) {{ hold kv (8) {{ run svc (1); }} cache (8); }}
                run gate (2);                                   // t = 3: queue for the slot
                hold slot (1) {{ hold kv (8) {{ observe hit0 = cached >= 8; run svc (0.1); }} cache (0); }}
              }}
              branch (serial == 1) {{
                run gate (1);
                hold slot (1) {{ hold kv (10) {{ run svc (1); }} cache (10); }}
                run gate (5);                                   // tool call t = 2..7
                hold slot (1) {{ hold kv (10) {{ observe hit1 = cached >= 10; run svc (0.1); }} cache (0); }}
              }}
              branch (serial == 2) {{
                run gate (3.5);
                hold kv (20) {{ run svc (0.1); }}               // no slot needed: evicts at t = 3.5
              }}
              branch (serial == 3) {{
                run gate (2);
                hold slot (1) {{ run gate (2); }}               // blocks the slot t = 2..4
              }}
              end;
            }}
            run {{ horizon 100; }}
            "#
        );
        let r = run(&src);
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
        workload { arrive batch(2); }
        session {
          run gate (serial);
          // s0 takes 55 -> 60 allocated, caches 55 -> 50 (five full blocks)
          // s1 takes 70 -> needs 70 of 100 - 0 used; cached 50 -> evict 2 blocks
          hold kv (serial == 0 ? 55 : 70) { observe used = used(kv); run svc (1); } cache (serial == 0 ? 55 : 0);
          run gate (10);
          branch (serial == 0) { hold kv (55) { observe cached0 = cached; run svc (0.1); } cache (0); }
          end;
        }
        run { horizon 100; }
    "#;
    let r = run(src);
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
        workload { arrive batch(2); init { set c = serial == 0 ? 20 : 25; } }
        session {
          run gate (serial);
          hold kv (c) { run svc (1); } cache (c);
          run gate (5);
          branch (serial == 0) {
            observe in_tier = cachedin(tier);
            branch (cachedin(tier) > 0) { run link (cachedin(tier) / 100); observe fetched = 1; drop tier; }
            hold kv (c) { run svc (0.1); }
          }
          end;
        }
        run { horizon 100; }
    "#;
    let r = run(src);
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
            workload {{ arrive batch(3); init {{ set pri = serial == 2 ? 0 : 1; }} }}
            session {{
              branch (serial == 0) {{
                hold kv (6) {{ run svc (0.5); }} cache (6);
                run gate (0.6);
                hold kv (20) {{ run svc (0.1); }}
              }}
              branch (serial == 1) {{ run gate (0.5); hold kv (24) {{ run svc (10); }} }}
              branch (serial == 2) {{ run gate (1.2); hold kv (6) {{ run svc (1); }} }}
              end;
            }}
            run {{ horizon 100; }}"
        )
    };
    let r = run(&prog("waiting"));
    assert_eq!(r.pool("kv").unwrap().spills, 1, "{}", r.text());
    assert_eq!(r.pool("kv").unwrap().evicted_entries, 1, "{}", r.text());
    let r = run(&prog("!waiting"));
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
        workload { arrive batch(2); }
        session {
          run gate (serial);
          hold kv (50) {
            run svc (5);
            branch (serial == 0) { set t = now; grow kv (30); observe waited = now - t; }
          }
          observe done = now;
          end;
        }
        run { horizon 100; }
    "#;
    let r = run(src);
    // s0 asks for 30 more at t = 5 while s1 holds 50 until t = 6: waits 1
    assert_eq!(
        r.observe("waited").unwrap().samples,
        vec![1.0],
        "{}",
        r.text()
    );
    assert_eq!(r.pool("kv").unwrap().preemptions, 0);
}

/// `queue by (key)`: a priority queue admits the smallest key first.
#[test]
fn priority_queue_orders_admissions() {
    let src = r#"
        pool kv { cap 10; queue by (prio); }
        stage svc : fifo;
        workload { arrive batch(3); init { set prio = 2 - serial; } }
        session {
          hold kv (10) { observe order = serial; run svc (1); }
          end;
        }
        run { horizon 100; }
    "#;
    let r = run(src);
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
        workload { arrive batch(2); }
        session { hold kv (serial == 0 ? 20 : 5) { run svc (1); } observe done = serial; end; }
        run { horizon 100; }
    "#;
    let r = run(src);
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
        workload { arrive batch(3); }
        session { hold kv (1) reserve (serial == 0 ? 20 : 1) { run svc (1); } observe done = serial; end; }
        run { horizon 100; }
    "#;
    let r = run(src);
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
        "hold kv (20)",
        "hold kv (2 * 5 + 1)",
        "hold kv (1) reserve (11)",
        "hold kv (max(20, 1))",
        "hold kv (1 > 0 ? 20 : 1)",
        "hold kv2[0] (20)",
        "hold kv2[serial] (20)",
    ] {
        let src = format!(
            "pool kv {{ cap 10; }} pool kv2[2] {{ cap 10; }} stage d : delay;
             workload {{ arrive batch(1); }}
             session {{ {hold} {{ run d (1); }} end; }}
             run {{ horizon 10; }}"
        );
        let e = run_source(&src, &Overrides::default(), None).unwrap_err();
        let said = if hold.contains("serial") {
            "more than the cap of every member (`kv2`: 10)"
        } else {
            "more than its cap 10"
        };
        assert!(e.contains(said), "{hold}: {e}");
    }
    let e = run_source(
        "pool kv { cap 10; block 4; } stage d : delay;
         workload { arrive batch(1); }
         session { hold kv (9) { run d (1); } end; }
         run { horizon 10; }",
        &Overrides::default(),
        None,
    )
    .unwrap_err();
    assert!(
        e.contains("waits for 9 units (12 in blocks of 4), more than its cap 10"),
        "{e}"
    );
    // units the program computes are the run's: the session ends, counted
    // in `rej`, and the report says so
    let r = run("pool kv { cap 10; } stage d : delay;
         workload { arrive batch(2); init { set u = 5 + 10 * serial; } }
         session { hold kv (u) { run d (1); } end; }
         run { horizon 10; }");
    assert_eq!(r.pool("kv").unwrap().rejected, 1, "{}", r.text());
    assert!(
        r.text()
            .contains("rej: 1 session(s) ended at pool `kv` asking for more than its cap"),
        "{}",
        r.text()
    );
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
        pool reqs { cap 4; admit via engine; }
        pool kv { cap 160; block 16; evict lru; preempt lifo; }
        stage engine : step { budget 1000; chunk 0; cost 1; memory kv; }
        workload { arrive batch(1); }
        session {
          hold reqs (1), kv (100) reserve (100) {
            run engine prefill (100) growing kv;
            run engine decode (100) growing kv;
          }
          end;
        }
        run { horizon 400; }
    "#;
    let r = run(src);
    let kv = r.pool("kv").unwrap();
    assert_eq!(kv.preemptions, 6, "{}", r.text());
    assert_eq!(kv.admissions, 7, "{}", r.text());
    assert_eq!(kv.stuck, 1, "{}", r.text());
    assert!(r.text().contains("stuck: 1 session(s)"), "{}", r.text());
    assert_eq!(r.ended, 0, "{}", r.text());
    // one step per unit of time, and the step that starts at the horizon
    // is counted when it starts
    assert_eq!(r.stage("engine").unwrap().iterations, 401, "{}", r.text());
}

/// The step that only preempted lasts `C` at zero tokens. With `cost tokens`
/// that is 0: the step ends at the same instant and the next one re-admits,
/// so a zero cost is one more event at that instant, not a loop. Prefill
/// 100 costs 100, sixty decodes cost 60, the preempting step 0: preemptions
/// at t = 160 and 320, the third prefill would end at 420 > 400.
#[test]
fn a_zero_cost_preempting_step_does_not_hang() {
    let src = r#"
        pool reqs { cap 4; admit via engine; }
        pool kv { cap 160; block 16; evict lru; preempt lifo; }
        stage engine : step { budget 1000; chunk 0; cost tokens; memory kv; }
        workload { arrive batch(1); }
        session {
          hold reqs (1), kv (100) reserve (100) {
            run engine prefill (100) growing kv;
            run engine decode (100) growing kv;
          }
          end;
        }
        run { horizon 400; }
    "#;
    let r = run(src);
    let kv = r.pool("kv").unwrap();
    assert_eq!(kv.preemptions, 2, "{}", r.text());
    assert_eq!(kv.stuck, 1, "{}", r.text());
}

/// A `branch` guard is a test, 0 or 1. A computed fraction used to be drawn
/// as a probability without anyone asking for a draw; now it is an error
/// when evaluated (a constant one is a link error, `tests/lints.rs`).
const GUARD: &str = "
    stage svc : delay;
    workload { arrive batch(1); init { set c = 5; set K = 10; } }
    session { branch (GUARD) { run svc (1); } end; }
    run { horizon 10; }";

#[test]
fn a_computed_fraction_is_not_a_draw() {
    let error = run_source(
        &GUARD.replace("GUARD", "c / K"),
        &Overrides::default(),
        None,
    )
    .unwrap_err();
    assert!(error.contains("the guard is 0.5, not 0 or 1"), "{error}");
}

#[test]
fn a_nan_guard_is_an_error() {
    let error = run_source(
        &GUARD.replace("GUARD", "0 / 0"),
        &Overrides::default(),
        None,
    )
    .unwrap_err();
    assert!(error.contains("the guard is NaN, not 0 or 1"), "{error}");
}

#[test]
fn a_negative_guard_is_an_error() {
    let error = run_source(
        &GUARD.replace("GUARD", "0 - 1"),
        &Overrides::default(),
        None,
    )
    .unwrap_err();
    assert!(error.contains("the guard is -1, not 0 or 1"), "{error}");
}

#[test]
fn a_boolean_guard_and_a_declared_draw_run() {
    run(&GUARD.replace("GUARD", "c < K"));
    run(&GUARD.replace("branch (GUARD)", "branch with (c / K)"));
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
        "hold kv (min(cachedin(kv), 992) + 8) at admission (hit = min(cachedin(kv), 992)) {
                     observe hit = cached > 0;
                     prefill on engine (1000 - cached) growing kv;
                   } cache (1000);";
    for (wrap, want_hit) in [
        (request.to_string(), 1.0),
        (format!("hold kv (0) {{ {request} }}"), 1.0),
        (format!("hold kv (0) {{ release kv; {request} }}"), 1.0),
        (format!("hold kv (0) {{ {request} }} cache (0);"), 0.0),
    ] {
        let src = format!(
            r#"
            pool kv {{ cap 100000; block 16; evict lru; }}
            stage engine : step {{ budget 8192; cost tokens * 1e-5 + 1e-4; memory kv; }}
            stage think : delay;
            workload {{ arrive closed(1); }}
            session {{ loop {{ run think (1); {wrap} }} }}
            run {{ horizon 20; warmup 0; seed 1; }}
            "#
        );
        let r = run(&src);
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
             workload {{ arrive batch(1); init {{ set z = 0; set i = 0; }} }}
             session {{ {stmt} end; }}
             run {{ horizon 10; }}"
        );
        run_source(&src, &Overrides::default(), None).unwrap_err()
    };
    for (stmt, said) in [
        ("run d (z / z);", "`run d (z / z)`: the amount is NaN"),
        ("run d (z - 5);", "`run d (z - 5)`: the amount is -5"),
        (
            "hold kv (z - 1) { run d (1); }",
            "`hold kv (z - 1)`: the amount is -1",
        ),
        (
            "hold kv (8) { grow kv (z - 1); }",
            "`grow kv (z - 1)`: the amount is -1",
        ),
        (
            "hold kv (8) { load kv (z / z); }",
            "`load kv (z / z)`: the amount is NaN",
        ),
        (
            "run a[i - 1] (1);",
            "index `i - 1` is -1: a member of an array of 2 is 0 to 1",
        ),
        ("run a[i + 0.5] (1);", "index `i + 0.5` is 0.5"),
        ("run a[i + 2] (1);", "index `i + 2` is 2"),
        ("hold q[i - 1] (1) { run d (1); }", "index `i - 1` is -1"),
        (
            "hold kv (1) reserve (z / z) { run d (1); }",
            "`hold kv reserve (z / z)`: the amount is NaN",
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
        ("run d (-5);", "`run d (-5)`"),
        ("hold kv (2 - 3) { run d (1); }", "`hold kv (-1)`"),
        ("hold kv (8) { grow kv (-1); }", "`grow kv (-1)`"),
        ("hold kv (8) { load kv (0 / 0); }", "`load kv (NaN)`"),
    ] {
        let src = format!(
            "pool kv {{ cap 64; }} stage d : delay;
             workload {{ arrive batch(1); }}
             session {{ {stmt} end; }}
             run {{ horizon 10; }}"
        );
        let e = serq::compile_source(&src, &Overrides::default()).unwrap_err();
        assert!(
            e.contains(said) && e.contains("is a number, and not negative"),
            "{stmt}: {e}"
        );
    }
    // a constant index names a member, in IR as in text
    let mut p = serq::compile_source(
        "stage a[2] : delay; workload { arrive batch(1); }
         session { run a[0] (1); end; } run { horizon 10; }",
        &Overrides::default(),
    )
    .unwrap();
    let serq::ir::CStmt::Run { stage, .. } = &mut p.blocks[p.session][0] else {
        panic!("the session runs first")
    };
    stage.index = Some(Box::new(serq::ir::CExpr::Num(-1.0)));
    let e = p.validate().unwrap_err();
    assert!(
        e.contains("stage index -1: a member of an array of 2 is 0 to 1"),
        "{e}"
    );
    // a decode is named as the kernel writes it (`decode on E (…)`)
    let e = run_source(
        "pool kv { cap 64; } stage eng : step { budget 8; cost 1; memory kv; }
         workload { arrive batch(1); init { set z = 0; } }
         session { hold kv (8) { run eng decode (z - 1); } end; }
         run { horizon 10; }",
        &Overrides::default(),
        None,
    )
    .unwrap_err();
    assert!(
        e.contains("`run eng decode (z - 1)`: the amount is -1"),
        "{e}"
    );
    // zero is an amount: a run of no work, a hold of nothing
    let r = run("pool kv { cap 64; } stage d : delay;
         workload { arrive batch(1); init { set z = 0; } }
         session { run d (z); hold kv (z) { run d (1); } end; }
         run { horizon 10; }");
    assert_eq!(r.ended, 1, "{}", r.text());
}

/// A hold takes each pool once. Twice, `grow` grew one entry, the pool's
/// `used` once, and the release gave both back: `used` went negative (#309).
#[test]
fn a_hold_takes_a_pool_once() {
    let src = "pool kv { cap 400; block 16; }
        stage engine : step { budget 128; chunk 128; cost 0.001; memory kv; }
        workload { arrive batch(3); }
        session {
          set prompt = 64;
          hold kv (16), kv (16) { prefill prompt growing kv; decode 40 growing kv; }
          end;
        }
        run { horizon 1; }";
    let e = serq::compile_source(src, &Overrides::default()).unwrap_err();
    assert!(e.contains("a hold takes `kv` twice"), "{e}");
    // indices the run sets to one member: only the run can tell
    let e = run_source(
        "pool kv[2] { cap 64; } stage d : delay;
         workload { arrive batch(1); init { set i = 1; set j = 1; } }
         session { hold kv[i] (1), kv[j] (1) { run d (1); } end; }
         run { horizon 10; }",
        &Overrides::default(),
        None,
    )
    .unwrap_err();
    assert!(
        e.contains("`kv[i]` and `kv[j]` name the same member, `kv[1]`"),
        "{e}"
    );
}
