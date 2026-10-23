//! Deterministic checks of the pool semantics: admission with eviction,
//! eviction orders, block-level caches, spilling to a tier, growth with
//! and without preemption, priority queues.

mod common;

use serq::Overrides;

fn run(src: &str, workload: &str, options: &Overrides) -> serq::Report {
    common::run(&common::main_source(src), workload, options).unwrap()
}

/// Three sessions of one turn each.
const THREE: &str = "workload { arrive batch(3);
          session { turn;
            end;

          }
        }";

/// One session of one turn, the workload most of these programs run.
const ONE: &str = "workload { arrive batch(1);
          session { turn; end;
          }
        }";

/// `holders(p)` counts the sessions holding `p`: a session with a hold
/// nested in another on the same pool is one, and stays one when the inner
/// hold ends (it read 0 then, while the outer hold still held `kv`).
#[test]
fn an_inner_hold_leaves_the_outer_one_holding() {
    let src = "pool kv { cap 100; }
        stage svc : delay;
        server {
          hold kv (cost(kv, 10)) {
            hold kv (cost(kv, 10)) { observe both = holders(kv); run svc (cost(svc, 1)); }
            observe after_inner = holders(kv);
            run svc (cost(svc, 1));
          }
        }";
    let r = run(src, "workload { arrive batch(1); }", &common::horizon(10.0));
    // one session, holding the pool once or twice
    assert_eq!(r.observe("both").unwrap().samples, [1.0]);
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
        server (c) {{
          run gate (cost(gate, serial));                 // 0, 1, 2: sequential first turns
          hold kv (cost(kv, c)) {{ run svc (cost(svc, 0.1)); }} cache (cost(kv, c));
          run gate (cost(gate, 10));
          if (serial == 1) {{
            hold kv (cost(kv, c)) {{ observe hit = cached >= c; run svc (cost(svc, 0.1)); }} cache (cost(kv, c));
          }}
        }}

"#
        );
        let workload = "workload { arrive batch(3); init { set c = 10 * (serial + 1); } send (c);
          session { turn;
            end;

          }
        }";
        let r = run(&src, workload, &common::horizon(100.0));
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
        server {{
          if (serial == 0) {{
            hold slot (cost(slot, 1)) {{ hold kv (cost(kv, 8)) {{ run svc (cost(svc, 1)); }} cache (cost(kv, 8)); }}
            run gate (cost(gate, 2));                                   // t = 3: queue for the slot
            hold slot (cost(slot, 1)) {{ hold kv (cost(kv, 8)) {{ observe hit0 = cached >= 8; run svc (cost(svc, 0.1)); }} cache (cost(kv, 0)); }}
          }}
          if (serial == 1) {{
            run gate (cost(gate, 1));
            hold slot (cost(slot, 1)) {{ hold kv (cost(kv, 10)) {{ run svc (cost(svc, 1)); }} cache (cost(kv, 10)); }}
            run gate (cost(gate, 5));                                   // tool call t = 2..7
            hold slot (cost(slot, 1)) {{ hold kv (cost(kv, 10)) {{ observe hit1 = cached >= 10; run svc (cost(svc, 0.1)); }} cache (cost(kv, 0)); }}
          }}
          if (serial == 2) {{
            run gate (cost(gate, 3.5));
            hold kv (cost(kv, 20)) {{ run svc (cost(svc, 0.1)); }}               // no slot needed: evicts at t = 3.5
          }}
          if (serial == 3) {{
            run gate (cost(gate, 2));
            hold slot (cost(slot, 1)) {{ run gate (cost(gate, 2)); }}               // blocks the slot t = 2..4
          }}
        }}

"#
        );
        let workload = "workload { arrive batch(4);
          session { turn;
            end;

          }
        }";
        let r = run(&src, workload, &common::horizon(100.0));
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
        server {
          run gate (cost(gate, serial));
          // s0 takes 55 -> 60 allocated, caches 55 -> 50 (five full blocks)
          // s1 takes 70 -> needs 70 of 100 - 0 used; cached 50 -> evict 2 blocks
          hold kv (cost(kv, serial == 0 ? 55 : 70)) { observe used = used(kv); run svc (cost(svc, 1)); } cache (cost(kv, serial == 0 ? 55 : 0));
          run gate (cost(gate, 10));
          if (serial == 0) { hold kv (cost(kv, 55)) { observe cached0 = cached; run svc (cost(svc, 0.1)); } cache (cost(kv, 0)); }
        }

"#;
    let workload = "workload { arrive batch(2);
          session { turn;
            end;

          }
        }";
    let r = run(src, workload, &common::horizon(100.0));
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
        server (c) {
          run gate (cost(gate, serial));
          hold kv (cost(kv, c)) { run svc (cost(svc, 1)); } cache (cost(kv, c));
          run gate (cost(gate, 5));
          if (serial == 0) {
            observe in_tier = cachedin(tier);
            if (cachedin(tier) > 0) { run link (cost(link, cachedin(tier) / 100)); observe fetched = 1; drop tier; }
            hold kv (cost(kv, c)) { run svc (cost(svc, 0.1)); }
          }
        }

"#;
    let workload = "workload { arrive batch(2); init { set c = serial == 0 ? 20 : 25; } send (c);
          session { turn;
            end;

          }
        }";
    let r = run(src, workload, &common::horizon(100.0));
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

/// A spill's copy is a job no session owns, with no tokens and no mode:
/// it goes over a stage, not an engine, whose iteration serves a request's
/// prefill or decode. Spilled via an engine, the run panicked at the
/// engine's first iteration after the spill (`give`: `RunMode::Plain =>
/// unreachable!()`, #733); it does not link. Over a stage: B's prompt of
/// 4 does not fit beside A's cached prompt of 2 (its one output token is
/// the prefill's, with no KV) in a `kv` of 5, so B's admission evicts A's
/// entry, 2 units, which spills: one spill, one copy done on `link`.
#[test]
fn a_spill_goes_over_a_stage_not_an_engine() {
    let program = |via: &str, link: &str| {
        format!(
            "device gpu {{ kv cap 5; }}
        engine llm on gpu {{
          tokens cap 8;
          scheduler {{ advance running; admit waiting while (running.preempted == 0) taking kv (request.input) cache (100); }}
          worker {{ forward (1); }}
        }}
        pool l2 {{ cap 100; }}{link}
        pool kv on gpu {{ evict lru; spill l2 via {via} (3) when (1); }}
        server (prompt) {{ llm (prompt, 1); }}"
        )
    };
    let workload =
        "workload { arrive batch(2); init { set prompt = serial == 0 ? 2 : 4; } send (prompt);
          session { turn; end; }
        }";
    let Err(error) = common::compile(
        &common::main_source(&program("llm", "")),
        workload,
        &common::horizon(20.0),
    ) else {
        panic!("a spill via an engine links");
    };
    assert!(
        error.contains(
            "pool `kv`: `spill l2 via llm`: `llm` is an engine, whose iteration serves \
             requests' prefill and decode tokens, and a spill's copy is no request's"
        ),
        "{error}"
    );
    let r = run(
        &program("link", " stage link : delay;"),
        workload,
        &common::horizon(20.0),
    );
    let kv = r.pool("kv").unwrap();
    assert_eq!(
        (kv.evicted_entries, kv.evicted_units, kv.spills),
        (1, 2.0, 1),
        "{}",
        r.text()
    );
    assert_eq!(r.stage("link").unwrap().completed, 1, "{}", r.text());
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
        server (pri) {{
          if (serial == 0) {{
            hold kv (cost(kv, 6)) {{ run svc (cost(svc, 0.5)); }} cache (cost(kv, 6));
            run gate (cost(gate, 0.6));
            hold kv (cost(kv, 20)) {{ run svc (cost(svc, 0.1)); }}
          }}
          if (serial == 1) {{ run gate (cost(gate, 0.5)); hold kv (cost(kv, 24)) {{ run svc (cost(svc, 10)); }} }}
          if (serial == 2) {{ run gate (cost(gate, 1.2)); hold kv (cost(kv, 6)) {{ run svc (cost(svc, 1)); }} }}
        }}
        "
        )
    };
    let workload = "workload { arrive batch(3); init { set pri = serial == 2 ? 0 : 1; } send (pri);
          session { turn;
            end;

          }
        }";
    let r = run(&prog("waiting"), workload, &common::horizon(100.0));
    assert_eq!(r.pool("kv").unwrap().spills, 1, "{}", r.text());
    assert_eq!(r.pool("kv").unwrap().evicted_entries, 1, "{}", r.text());
    let r = run(&prog("!waiting"), workload, &common::horizon(100.0));
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
        server {
          run gate (cost(gate, serial));
          hold kv (cost(kv, 50)) {
            run svc (cost(svc, 5));
            if (serial == 0) { set t = now; grow kv (cost(kv, 30)); observe grow_wait = now - t; }
          }
          observe done = now;
        }

"#;
    let workload = "workload { arrive batch(2);
          session { turn;
            end;

          }
        }";
    let r = run(src, workload, &common::horizon(100.0));
    // s0 asks for 30 more at t = 5 while s1 holds 50 until t = 6: waits 1
    assert_eq!(
        r.observe("grow_wait").unwrap().samples,
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
        server {
          if (serial == 0) {
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
    let workload = "workload { arrive batch(2);
          session { turn;
            end;

          }
        }";
    let r = run(
        src,
        workload,
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
        server (prio) {
          hold kv (cost(kv, 10)) { observe order = serial; run svc (cost(svc, 1)); }
        }

"#;
    let workload = "workload { arrive batch(3); init { set prio = 2 - serial; } send (prio);
          session { turn;
            end;

          }
        }";
    let r = run(src, workload, &common::horizon(100.0));
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
        server { hold kv (cost(kv, serial == 0 ? 20 : 5)) { run svc (cost(svc, 1)); } observe done = serial;
        }

"#;
    let workload = "workload { arrive batch(2);
          session { turn; end;
          }
        }";
    let r = run(src, workload, &common::horizon(100.0));
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
        server { hold kv (cost(kv, 1)) reserve (cost(kv, serial == 0 ? 20 : 1)) { run svc (cost(svc, 1)); } observe done = serial;
        }

"#;
    let workload = "workload { arrive batch(3);
          session { turn; end;
          }
        }";
    let r = run(src, workload, &common::horizon(100.0));
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
        server {{ {hold} {{ run d (cost(d, 1)); }}
        }}
        "
        );
        let e = common::run(&common::main_source(&src), ONE, &common::horizon(10.0)).unwrap_err();
        let said = if hold.contains("serial") {
            "more than the cap of every member (`kv2`: 10)"
        } else {
            "more than its cap 10"
        };
        assert!(e.contains(said), "{hold}: {e}");
    }
    let e = common::run(
        &common::main_source(
            "pool kv { cap 10; block 4; } stage d : delay;
        server { hold kv (cost(kv, 9)) { run d (cost(d, 1)); }
        }
        ",
        ),
        ONE,
        &common::horizon(10.0),
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
        server (u) { hold kv (cost(kv, u)) { run d (cost(d, 1)); }
        }
        ",
        "workload { arrive batch(2); init { set u = 5 + 10 * serial; } send (u);
          session { turn; end;
          }
        }",
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
/// stuck (#418). The text can no longer say it (#439: a pool is admitted
/// by the engine its `on` names), so the IR says it and is refused.
#[test]
fn admit_via_names_a_step_stage() {
    for (stage, kind) in [
        ("stage F : fifo;", "a fifo stage"),
        ("stage F : ps(2);", "a ps stage"),
        ("stage F : delay;", "a delay stage"),
    ] {
        let src = format!(
            "pool reqs {{ cap 4; }}
  {stage}
  server (t0) {{
    hold reqs (cost(reqs, 1)) {{ run F (cost(F, 1)); }}
    observe response = now - t0;
  }}"
        );
        let workload = "workload { arrive poisson(1); init { set t0 = now; } send (t0); }";
        let mut p = common::compile(
            &common::main_source(&src),
            workload,
            &common::horizon(100.0),
        )
        .unwrap();
        let f = p.stages.iter().position(|s| s.name == "F").unwrap();
        let reqs = p.pools.iter().position(|q| q.name == "reqs").unwrap();
        p.pools[reqs].admit_via = Some(f);
        let error = p.validate().unwrap_err();
        assert!(
            error.contains(&format!(
                "`F` is {kind}: only an engine's `scheduler` admits"
            )),
            "{error}"
        );
    }
}

/// The engine of the livelock below, with the pool's `preempt` clause.
fn outgrowing(preempt: &str) -> String {
    format!(
        r#"
        device gpu {{ kv cap 160; }}
        engine llm on gpu {{
          reqs cap 4;
          tokens cap 1000;
          scheduler {{
            advance running;
            admit waiting while (running.preempted == 0) taking reqs (1), kv (100) reserve (100);
          }}
          worker {{ forward (1); }}
        }}
        pool reqs on llm {{ }}
        pool kv on gpu {{ block 16; evict lru;{preempt} }}
        server {{ llm (100, 101); }}
"#
    )
}

/// A request that fits at admission but can never grow to what it needs
/// preempts itself, re-enters at the head of the queue, and does it again:
/// a livelock the run would otherwise hide behind a preemption count. The
/// report counts the session as `outgrown` (its hold asked the pool for
/// more than the cap), and the run is `blocked`; it is `stuck` too
/// (preempted again without passing its position), which is not the
/// verdict (#64: thrash is stuck as well, `a_thrashing_run_is_not_blocked`).
///
/// On 10 blocks of 16 (160 tokens) with a 1000-token budget: step 1
/// prefills the 100-token prompt (7 blocks), steps 2..61 decode tokens
/// 101..160 (block 8 at 113, 9 at 129, 10 at 145), step 62 needs an 11th
/// block: 160 + 16 > 160, the growth past the cap that `outgrown` counts.
/// None is free, the request is `running[-1]` and preempts itself: the
/// step schedules nothing and is not skipped (vLLM's `schedule()` runs it
/// and admits nothing, scheduler.py:869). The request keeps the 61 tokens
/// it generated (scheduler.py:1561): it has 161, more than the pool, so
/// from step 63 every step re-admits it and its prefill preempts it at
/// once: 340 preemptions and admissions before the horizon of 400. It
/// waits for nobody, so nothing is `deadlocked`.
#[test]
fn a_hold_that_can_never_fit_is_reported_outgrown() {
    let r = run(&outgrowing(" preempt lifo;"), ONE, &common::horizon(400.0));
    let kv = r.pool("kv").unwrap();
    assert_eq!(kv.preemptions, 340, "{}", r.text());
    assert_eq!(kv.admissions, 340, "{}", r.text());
    assert_eq!(kv.stuck, 1, "{}", r.text());
    assert_eq!((kv.outgrown, kv.deadlocked), (1, 0), "{}", r.text());
    assert!(kv.blocked() && r.blocked(), "{}", r.text());
    assert!(r.text().contains("outgrown: 1 session(s)"), "{}", r.text());
    assert!(r.text().contains("stuck: 1 session(s)"), "{}", r.text());
    assert_eq!(r.ended, 0, "{}", r.text());
    // one step per unit of time, and the step that starts at the horizon
    // is counted when it starts
    assert_eq!(r.stage("llm").unwrap().iterations, 401, "{}", r.text());
}

/// The same request under `preempt none`: at step 62 its growth waits, for
/// room only its own hold keeps. It is `outgrown` and `deadlocked` both,
/// its growth `growing_stalled`; nothing is preempted, so nothing is
/// `stuck`, which the run's verdict never read.
#[test]
fn a_hold_that_can_never_fit_waits_for_ever_without_preempt() {
    let r = run(&outgrowing(""), ONE, &common::horizon(400.0));
    let kv = r.pool("kv").unwrap();
    assert_eq!(kv.preemptions, 0, "{}", r.text());
    assert_eq!(kv.stuck, 0, "{}", r.text());
    assert_eq!((kv.outgrown, kv.deadlocked), (1, 1), "{}", r.text());
    assert_eq!(kv.growing_at_end, 1, "{}", r.text());
    assert!(kv.growing_stalled, "{}", r.text());
    assert!(r.blocked(), "{}", r.text());
    assert!(r.text().contains("deadlock: 1 session(s)"), "{}", r.text());
    assert_eq!(r.ended, 0, "{}", r.text());
}

/// Two sessions take two pools of one unit each in opposite orders: at
/// t = 1 each holds one and queues for the other, which the other holds. No
/// event releases either (no lease, no preemption): both are `deadlocked`,
/// one at each pool, and the run is `blocked`. Before #64 the run ended
/// with `ended 0`, no note and exit 0. With two units each, the same
/// program ends both sessions: a linker could not tell the two apart
/// (`docs/design/push-mode.md`, the same call for a send's cycle).
#[test]
fn holds_taken_in_opposite_orders_deadlock() {
    let src = |cap: u32| {
        format!(
            "pool a {{ cap {cap}; }}
        pool b {{ cap {cap}; }}
        stage svc : delay;
        server (x) {{
          if (x) {{
            hold a (cost(a, 1)) {{ run svc (cost(svc, 1)); hold b (cost(b, 1)) {{ run svc (cost(svc, 1)); }} }}
          }} else {{
            hold b (cost(b, 1)) {{ run svc (cost(svc, 1)); hold a (cost(a, 1)) {{ run svc (cost(svc, 1)); }} }}
          }}
        }}"
        )
    };
    let workload = "workload { arrive batch(2);
          init { set x = serial == 0 ? 1 : 0; }
          send (x);
        }";
    let r = run(&src(1), workload, &common::horizon(100.0));
    assert_eq!(r.ended, 0, "{}", r.text());
    for name in ["a", "b"] {
        let q = r.pool(name).unwrap();
        assert_eq!((q.deadlocked, q.outgrown), (1, 0), "{name}: {}", r.text());
    }
    assert!(r.blocked(), "{}", r.text());
    assert!(
        r.text().contains("deadlock: 1 session(s) wait at pool `a`"),
        "{}",
        r.text()
    );
    let r = run(&src(2), workload, &common::horizon(100.0));
    assert_eq!(r.ended, 2, "{}", r.text());
    assert!(!r.blocked(), "{}", r.text());
}

/// The crossing holds of `holds_taken_in_opposite_orders_deadlock`, the
/// first two ways of `server (x)`; `third` is the way of `x == 2`.
fn crossing(a: &str, ask_a: &str, third: &str) -> String {
    format!(
        "pool a {{ cap 1; {a} }}
        pool b {{ cap 1; }}
        stage svc : delay;
        server (x) {{
          if (x == 2) {{ {third} }} else {{
            if (x) {{
              hold a (cost(a, 1)) {{ run svc (cost(svc, 1)); hold b (cost(b, 1)) {{ run svc (cost(svc, 1)); }} }}
            }} else {{
              hold b (cost(b, 1)) {{ run svc (cost(svc, 1)); hold a (cost(a, {ask_a})) {{ run svc (cost(svc, 1)); }} }}
            }}
          }}
        }}"
    )
}

/// A deadlock only as the hold first asked: session 1 asks `a` for
/// `now < 50 ? 1 : 0`, which reads the clock, and the queue's ticks try it
/// again every 10 s. Session 0 holds `a` from the tick at 10 and waits for
/// `b`, which session 1 holds; at the tick at 50 session 1's hold asks 0,
/// is admitted, and both end. At horizon 40 both wait, but no run is
/// blocked: an ask that reads the clock, a draw or the state may be lower
/// at a later try (`ir::moves`, the rule a rejection reads), so it is not
/// judged. Judged by what it asked when it queued, the run was called
/// deadlocked at 40.
#[test]
fn a_hold_whose_ask_moves_is_not_deadlocked() {
    let src = crossing("queue fifo every (10);", "now < 50 ? 1 : 0", "");
    let workload = "workload { arrive batch(2);
          init { set x = serial == 0 ? 1 : 0; }
          send (x);
        }";
    let r = run(&src, workload, &common::horizon(40.0));
    assert_eq!(r.ended, 0, "{}", r.text());
    assert!(!r.blocked(), "{}", r.text());
    let r = run(&src, workload, &common::horizon(100.0));
    assert_eq!(r.ended, 2, "{}", r.text());
}

/// A deadlock a later admission breaks: `a` preempts the first it admitted
/// (`preempt by (admission)`), whose hold queues again as a newcomer, and
/// serves its queue by the highest serial. At t = 1 sessions 0 and 1 wait
/// for each other as above; at t = 50 session 2 is admitted to `a` with
/// nothing, grows by 1 and preempts session 0's hold of `a`, whose wait
/// for `b` ends with it. At 51 session 2 ends, session 1 takes `a` and
/// ends at 52, and session 0 takes both pools and ends. So at horizon 40
/// the run is not blocked: a pool whose `preempt` may take one of its
/// holders (`candidates`, `lessees`) may free room at a growth there.
#[test]
fn a_deadlock_a_later_growth_may_break_is_not_blocked() {
    let src = crossing(
        "queue by (-serial); preempt by (admission) requeue tail;",
        "1",
        "run svc (cost(svc, 50)); hold a (cost(a, 0)) { grow a (cost(a, 1)); run svc (cost(svc, 1)); }",
    );
    let workload = "workload { arrive batch(3);
          init { set x = serial == 0 ? 1 : (serial == 1 ? 0 : 2); }
          send (x);
        }";
    let r = run(&src, workload, &common::horizon(40.0));
    assert_eq!(r.ended, 0, "{}", r.text());
    assert!(!r.blocked(), "{}", r.text());
    let r = run(&src, workload, &common::horizon(100.0));
    assert_eq!(r.ended, 3, "{}", r.text());
    assert_eq!(r.pool("a").unwrap().preemptions, 1, "{}", r.text());
}

/// Three requests of 60 + 60 tokens on one 160-token pool, admitted with
/// what the step's budget of 32 tokens takes of their prompt: they preempt
/// one another, two of them again below their previous position (`stuck`
/// 2), and all three end. Each needs 8 blocks of the 10 at most, so none
/// is `outgrown`: thrash is no verdict (`tests/cli.rs`, exit 0).
#[test]
fn a_thrashing_run_is_not_blocked() {
    let src = r#"
        device gpu { kv cap 160; }
        engine llm on gpu {
          reqs cap 4;
          tokens cap 32;
          scheduler {
            advance running;
            admit waiting while (running.preempted == 0)
              taking reqs (1), kv (min(request.known, budget_left(llm))) reserve (16);
          }
          worker { forward (1); }
        }
        pool reqs on llm { }
        pool kv on gpu { block 16; evict lru; preempt lifo; }
        server { llm (60, 60); }
"#;
    let r = run(src, THREE, &common::horizon(2000.0));
    let kv = r.pool("kv").unwrap();
    assert_eq!(r.ended, 3, "{}", r.text());
    assert_eq!(kv.stuck, 2, "{}", r.text());
    assert_eq!((kv.outgrown, kv.deadlocked), (0, 0), "{}", r.text());
    assert!(!r.blocked(), "{}", r.text());
}

/// The step that only preempted lasts `C` at zero tokens. With `cost tokens`
/// that is 0: the step ends at the same instant and the next one re-admits,
/// so a zero cost is one more event at that instant, not a loop. Prefill
/// 100 costs 100, sixty decodes cost 60, the preempting step 0: preemptions
/// at t = 160 and 320, the third prefill would end at 420 > 400.
#[test]
#[ignore = "#555: the resumed request keeps its 61 generated tokens, 161 on a \
            160-token pool, so every step preempts at zero cost and the run \
            spins at one instant; the interpreter should fail loudly"]
fn a_zero_cost_preempting_step_does_not_hang() {
    let src = r#"
        device gpu { kv cap 160; }
        engine llm on gpu {
          reqs cap 4;
          tokens cap 1000;
          scheduler {
            advance running;
            admit waiting while (running.preempted == 0) taking reqs (1), kv (100) reserve (100);
          }
          worker { forward (batch.tokens); }
        }
        pool reqs on llm { }
        pool kv on gpu { block 16; evict lru; preempt lifo; }
        server { llm (100, 101); }

"#;
    let workload = "workload { arrive batch(1);
          session { turn;
            end;

          }
        }";
    let r = run(src, workload, &common::horizon(400.0));
    let kv = r.pool("kv").unwrap();
    assert_eq!(kv.preemptions, 2, "{}", r.text());
    assert_eq!(kv.stuck, 1, "{}", r.text());
}

/// An `if` guard is a test, 0 or 1. A computed fraction used to be drawn
/// as a probability without anyone asking for a draw; now it is an error
/// when evaluated (a constant one is a link error, `tests/lints.rs`).
const GUARD: &str = "
        stage svc : delay;
        server (c, K) { if (GUARD) { run svc (cost(svc, 1)); }
        }
        ";
const GUARD_WORKLOAD: &str =
    "workload { arrive batch(1); init { set c = 5; set K = 10; } send (c, K);
          session { turn; end;
          }
        }";

#[test]
fn a_computed_fraction_is_not_a_draw() {
    let error = common::run(
        &common::main_source(&GUARD.replace("GUARD", "c / K")),
        GUARD_WORKLOAD,
        &common::horizon(100.0),
    )
    .unwrap_err();
    assert!(error.contains("the guard is 0.5, not 0 or 1"), "{error}");
}

#[test]
fn a_nan_guard_is_an_error() {
    let error = common::run(
        &common::main_source(&GUARD.replace("GUARD", "0 / 0")),
        GUARD_WORKLOAD,
        &common::horizon(100.0),
    )
    .unwrap_err();
    assert!(error.contains("the guard is NaN, not 0 or 1"), "{error}");
}

#[test]
fn a_negative_guard_is_an_error() {
    let error = common::run(
        &common::main_source(&GUARD.replace("GUARD", "0 - 1")),
        GUARD_WORKLOAD,
        &common::horizon(100.0),
    )
    .unwrap_err();
    assert!(error.contains("the guard is -1, not 0 or 1"), "{error}");
}

#[test]
fn a_boolean_guard_and_a_declared_draw_run() {
    run(
        &GUARD.replace("GUARD", "c < K"),
        GUARD_WORKLOAD,
        &common::horizon(100.0),
    );
    run(
        &GUARD.replace("if (GUARD)", "if (~bernoulli(c / K))"),
        GUARD_WORKLOAD,
        &common::horizon(100.0),
    );
}

/// #230: a hold without a `cache` clause takes no part in the prefix
/// cache. One session sends the same 1000-token prompt every second; the
/// request's admission keeps `cache (1000)`, i.e. 992 (62 full blocks of 16).
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
    let request = "llm (1000, 1);";
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
          scheduler {{
            advance running;
            admit waiting while (running.preempted == 0)
              taking kv (request.hit + 8) reuse (992) cache (1000);
          }}
          worker {{ forward (batch.tokens * 1e-5 + 1e-4); }}
        }}
        pool kv on gpu {{ block 16; evict lru; }}
        stage user : delay;
        server {{ loop {{ run user (cost(user, 1)); {wrap} }}
        }}

"#
        );
        let r = run(
            &src,
            "workload { arrive closed(1);
          session { turn;
          }
        }",
            &Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(20.0)
            },
        );
        // whether an admission read any of the cache
        let hits: Vec<f64> = r
            .observe("llm.reused")
            .unwrap()
            .samples
            .iter()
            .map(|&c| f64::from(u8::from(c > 0.0)))
            .collect();
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
    const ZERO: &str = "workload { arrive batch(1); init { set z = 0; } send (z);
          session { turn; end;
          }
        }";
    let fail = |stmt: &str| {
        // a stage no statement runs does not link: those `stmt` runs
        let d = if stmt.contains("run d ") {
            "stage d : delay;"
        } else {
            ""
        };
        let a = if stmt.contains("run a[") {
            "stage a[2] : delay;"
        } else {
            ""
        };
        let src = format!(
            "pool kv {{ cap 64; }} pool q[2] {{ cap 64; }} {d} {a}
        server (z, i) {{ {stmt}
        }}
        "
        );
        let workload = "workload { arrive batch(1); init { set z = 0; set i = 0; } send (z, i);
          session { turn; end;
          }
        }";
        common::run(&common::main_source(&src), workload, &common::horizon(10.0)).unwrap_err()
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
            // (a token count is whole, so its NaN is under `floor`, #661)
            "hold kv (cost(kv, 8)) { load kv (cost(kv, floor(z / z))); }",
            "`load kv (cost(kv, floor(z / z)))`: the amount is NaN",
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
        server {{ {stmt}
        }}
        "
        );
        let e =
            common::compile(&common::main_source(&src), ONE, &common::horizon(10.0)).unwrap_err();
        assert!(
            e.contains(said) && e.contains("is a number, and not negative"),
            "{stmt}: {e}"
        );
    }
    // a constant index names a member, in IR as in text
    let mut p = common::compile(
        &common::main_source(
            "stage a[2] : delay;
        server { run a[0] (cost(a, 1));
        } ",
        ),
        ONE,
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
    // a request's output is named as the call writes it
    let e = common::run(
        &common::main_source(
            "device gpu { kv cap 64; }
        engine eng on gpu {
          tokens cap 8;
          scheduler { advance running; admit waiting while (running.preempted == 0); }
          worker { forward (1); }
        }
        pool kv on gpu { }
        server (z) { hold kv (cost(kv, 8)) { eng (0, z - 1); }
        }
        ",
        ),
        ZERO,
        &common::horizon(10.0),
    )
    .unwrap_err();
    assert!(e.contains("`eng output (z - 1)`: the amount is -1"), "{e}");
    // zero is an amount: a run of no work, a hold of nothing
    let r = run(
        "pool kv { cap 64; } stage d : delay;
        server (z) { run d (cost(d, z)); hold kv (cost(kv, z)) { run d (cost(d, 1)); }
        }
        ",
        ZERO,
        &common::horizon(10.0),
    );
    assert_eq!(r.ended, 1, "{}", r.text());
}

/// A hold takes each pool once. Twice, `grow` grew one entry, the pool's
/// `used` once, and the release gave both back: `used` went negative (#309).
/// An engine's admission, which grows what it takes, says so in its own
/// words; a hold of a pool no engine admits, around the engine's request,
/// in the kernel's.
#[test]
fn a_hold_takes_a_pool_once() {
    let three = "workload { arrive batch(3);
          session { turn;
            end;

          }
        }";
    let src = |taking: &str, server: &str| {
        format!(
            "device gpu {{ kv cap 400; }}
        engine llm on gpu {{
          tokens cap 128;
          scheduler {{
            advance running each at most (128);
            admit waiting while (running.preempted == 0) each at most (128){taking};
          }}
          worker {{ forward (0.001); }}
        }}
        pool kv on gpu {{ block 16; }}
        server {{
          set prompt = 64;
          {server}
        }}
        "
        )
    };
    let e = common::compile(
        &common::main_source(&src(" taking kv (16), kv (16)", "llm (prompt, 41);")),
        three,
        &common::horizon(1.0),
    )
    .unwrap_err();
    assert!(e.contains("`taking` names `kv` twice"), "{e}");
    let e = common::compile(
        &common::main_source(&src(
            "",
            "hold kv (cost(kv, 16)), kv (cost(kv, 16)) { llm (prompt, 41); }",
        )),
        three,
        &common::horizon(1.0),
    )
    .unwrap_err();
    assert!(e.contains("a hold takes `kv` twice"), "{e}");
    // indices the run sets to one member: only the run can tell
    let e = common::run(
        &common::main_source(
            "pool kv[2] { cap 64; } stage d : delay;
        server (i, j) { hold kv[i] (cost(kv, 1)), kv[j] (cost(kv, 1)) { run d (cost(d, 1)); }
        }
        ",
        ),
        "workload { arrive batch(1); init { set i = 1; set j = 1; } send (i, j);
          session { turn; end;
          }
        }",
        &common::horizon(1.0),
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
        server {{
          hold reqs (cost(reqs, 1)), kv (cost(kv, 2)) reserve (cost(kv, {reserve})) {{ run svc (cost(svc, 1 + serial)); }}
          observe done = serial;
        }}

"#
        )
    };
    let r = run(
        &prog("4 + 4 * holders(reqs)"),
        THREE,
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
        THREE,
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
        THREE,
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
          scheduler {{
            advance running;
            admit waiting while (running.preempted == 0)
              taking reqs (1), kv (4) reserve (10) cache (0);
          }}
          worker {{ forward (1); }}
        }}
        pool reqs on llm {{ }}
        pool kv on gpu {{ preempt lifo; {held} }}
        server {{ llm (4, 7); observe admitted_at = llm.admitted; }}

"#
        )
    };
    let once = run(
        &prog(""),
        THREE,
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(100.0)
        },
    );
    assert!(once.pool("kv").unwrap().preemptions > 0, "{}", once.text());
    let held = run(
        &prog("reserve held;"),
        THREE,
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(100.0)
        },
    );
    assert_eq!(held.pool("kv").unwrap().preemptions, 0, "{}", held.text());
    let admitted = &held.observe("admitted_at").unwrap().samples;
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
        server {{
          run gate (cost(gate, serial));
          if (serial == 0) {{
            hold kv (cost(kv, 1)) reserve (cost(kv, 5)) {{
              hold kv (cost(kv, 1)) reserve (cost(kv, 5)) {{ run gate (cost(gate, 10)); }}
              {after_inner}
            }}
          }}
          if (serial == 1) {{
            hold kv (cost(kv, {second})) {{ observe admitted_at = now; run gate (cost(gate, 1)); }}
          }}
        }}

"#
        )
    };
    let admitted = |src: &str| {
        let r = run(
            src,
            "workload { arrive batch(2);
          session { turn;
            end;

          }
        }",
            &Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(100.0)
            },
        );
        r.observe("admitted_at").unwrap().samples.clone()
    };
    // 2 + 8 + 2 = 12: admitted on arrival, at 1
    assert_eq!(admitted(&prog("", 2)), [1.0]);
    // serial 0 starts at 0: the inner hold ends at 10, and the outer one
    // still reserves 4 until 20 (counted by holders, it was lost at 10):
    // 1 + 4 + 8 = 13 > 12 until then
    assert_eq!(admitted(&prog("run gate (cost(gate, 10));", 8)), [20.0]);
}

/// A run ends at its start plus its work, not at the sum of its chunks
/// (#479). A prompt of 0.4 + 1.2 in chunks of 1 summed to an ulp short of
/// it: the cached extent and the tokens of a request preempted right after
/// its prefill read a finished prefill as unfinished. A token count is a
/// whole number now (#661), so that prompt does not link, and a whole one's
/// chunks sum to it exactly.
#[test]
fn a_run_ends_at_its_start_plus_its_work() {
    let workload = |sessions: u32| {
        format!("workload {{ arrive batch({sessions}); session {{ turn; end; }} }}")
    };
    let prog = |prompt: &str, cap: u32, cache: &str, body: &str| {
        format!(
            r#"
        device gpu {{ kv cap {cap}; }}
        engine llm on gpu {{
          reqs cap 2;
          tokens cap 4;
          scheduler {{
            advance running each at most (1);
            admit waiting each at most (1) taking reqs (1), kv (0){cache};
          }}
          worker {{ forward (1); }}
        }}
        pool reqs on llm {{ }}
        pool kv on gpu {{ preempt lifo; }}
        server {{
          set prompt = {prompt};
          {body}
        }}
"#
        )
    };
    // #479's prompt is refused: the language says what the run would have
    // read an ulp short
    let e = common::compile(
        &common::main_source(&prog("0.4 + 1.2", 10, " cache (100)", "llm (prompt, 1);")),
        &workload(1),
        &common::horizon(20.0),
    )
    .unwrap_err();
    assert!(
        e.contains("`prompt` is a number of tokens")
            && e.contains("`0.4 + 1.2`")
            && e.contains("#661"),
        "{e}"
    );
    // the cached extent: the position the prefill ended at
    let src = prog(
        "1 + 1",
        10,
        " cache (100)",
        "llm (prompt, 1);
         observe extent = cachedin(kv);",
    );
    let r = run(&src, &workload(1), &common::horizon(20.0));
    assert_eq!(r.observe("extent").unwrap().samples, [2.0]);
    // B ends its prefill at 2 with A and samples a token, and A's first
    // decode token preempts it (lifo): B is admitted again with its prompt
    // and that token, which it prefills again
    let src = prog("1 + 1", 4, "", "llm (prompt, 3);");
    let r = run(&src, &workload(2), &common::horizon(20.0));
    assert_eq!(r.ended, 2, "{}", r.text());
    assert!(r.pool("kv").unwrap().preemptions >= 1, "{}", r.text());
    assert_eq!(
        r.observe("llm.prefill").unwrap().samples.last(),
        Some(&3.0),
        "{}",
        r.text()
    );
}

/// A growth that stalled and resumes does not move its hold's position: the
/// tokens it stalled on move it once, when they are computed (#482). B's
/// decode stalls on a token's growth (the server holds 3 of 7 for 5) and
/// resumes when that hold ends; main counted the stalled token twice and B
/// ended at 7, not 2 + 4. The run's end is checked against the sum of its
/// chunks (a `debug_assert!` in the interpreter), so a position moved twice
/// mid-run fails there too.
#[test]
fn a_resumed_growth_moves_the_position_once() {
    let src = r#"
        device gpu { kv cap 7; }
        engine llm on gpu {
          reqs cap 2;
          tokens cap 4;
          scheduler {
            advance running;
            admit waiting taking reqs (1), kv (0) cache (100);
          }
          worker { forward (1); }
        }
        pool reqs on llm { }
        pool kv on gpu { }
        stage gate : delay;
        server {
          if (serial == 0) {
            hold kv (cost(kv, 3)) { run gate (cost(gate, 5)); }
          } else {
            llm (2, 5);
            observe extent = cachedin(kv);
          }
        }
"#;
    let r = run(
        src,
        "workload { arrive batch(2); session { turn; end; } }",
        &common::horizon(50.0),
    );
    assert_eq!(r.ended, 2, "{}", r.text());
    assert_eq!(r.observe("extent").unwrap().samples, [6.0], "{}", r.text());
}
