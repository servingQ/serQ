//! Deterministic checks of the pool semantics: admission with eviction,
//! eviction orders, block-level caches, spilling to a tier, growth with
//! and without preemption, priority queues.

use seq::{Overrides, run_source};

fn run(src: &str) -> seq::Report {
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
        ("evict by (queued, size);", [1.0]),
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
/// queue (`queued`), whatever their size: s0 (8 cached, queued for the
/// slot) survives and s1 (10 cached, in a tool call) is evicted when s2
/// needs 20 of a 30-token pool. Plain shortest-first would drop s0.
#[test]
fn queued_sessions_are_evicted_after_suspended_ones() {
    for (order, want0, want1) in [
        ("evict by (queued, size);", 1.0, 0.0),
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
                hold slot (1) {{ hold kv (8) {{ observe hit0 = cached >= 8; run svc (0.1); }} }}
              }}
              branch (serial == 1) {{
                run gate (1);
                hold slot (1) {{ hold kv (10) {{ run svc (1); }} cache (10); }}
                run gate (5);                                   // tool call t = 2..7
                hold slot (1) {{ hold kv (10) {{ observe hit1 = cached >= 10; run svc (0.1); }} }}
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
          branch (serial == 0) { hold kv (55) { observe cached0 = cached; run svc (0.1); } }
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

/// A hold that can never fit ends the session (vLLM: FINISHED_IGNORED).
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
    assert_eq!(kv.stuck, 1, "{}", r.text());
    assert!(r.text().contains("stuck: 1 session(s)"), "{}", r.text());
    assert_eq!(r.ended, 0, "{}", r.text());
    // one step per unit of time, and the step that starts at the horizon
    // is counted when it starts
    assert_eq!(r.stage("engine").unwrap().iterations, 401, "{}", r.text());
}

/// The step that only preempted lasts `C` at zero tokens. With `cost ntok`
/// that is 0: the step ends at the same instant and the next one re-admits,
/// so a zero cost is one more event at that instant, not a loop. Prefill
/// 100 costs 100, sixty decodes cost 60, the preempting step 0: preemptions
/// at t = 160 and 320, the third prefill would end at 420 > 400.
#[test]
fn a_zero_cost_preempting_step_does_not_hang() {
    let src = r#"
        pool reqs { cap 4; admit via engine; }
        pool kv { cap 160; block 16; evict lru; preempt lifo; }
        stage engine : step { budget 1000; chunk 0; cost ntok; memory kv; }
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
