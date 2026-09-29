//! Deterministic checks of `release` and `load`, the two statements a KV
//! transfer between instances is written with, and of the coupling they let
//! a prefill/decode program state: the decoder's queue backs up into the
//! prefiller's memory.

use seq::{Overrides, check_source, run_source};

fn run(src: &str) -> seq::Report {
    run_source(src, &Overrides::default(), None).unwrap()
}

fn samples(r: &seq::Report, name: &str) -> Vec<f64> {
    r.observe(name).unwrap().samples.clone()
}

/// A pool of 10, two sessions of 10. The first releases its units after one
/// second and goes on working for five more; the second is admitted at the
/// release, not at the end of the first's scope.
#[test]
fn release_frees_the_pool_before_the_scope_ends() {
    let src = r#"
        pool kv { cap 10; }
        stage svc : delay;
        workload { arrive batch(2); }
        session {
          run svc (serial);
          set t0 = now;
          hold kv (10) {
            observe admitted = now - t0;
            run svc (1);
            release kv;
            observe free_after = free(kv);
            run svc (5);
          }
          observe free_at_end = free(kv);
          end;
        }
        run { horizon 100; }
    "#;
    let r = run(src);
    // session 0 at 0; session 1 arrives at 1 and is admitted at the release
    // (not at 6): the pool is free for it and taken by it at once
    assert_eq!(samples(&r, "admitted"), [0.0, 0.0]);
    assert_eq!(samples(&r, "free_after"), [0.0, 10.0]);
    // the scope end has nothing left to give back: the units are not freed
    // twice (session 1 released at 2 and held nothing at 6)
    assert_eq!(samples(&r, "free_at_end"), [10.0, 10.0]);
}

/// The released allocation keeps `cache` units cached, as the scope end
/// would: the next hold of the same session finds them.
#[test]
fn release_caches_per_the_hold_clause() {
    let src = r#"
        pool kv { cap 100; }
        stage svc : delay;
        workload { arrive batch(1); }
        session {
          hold kv (8) { run svc (1); release kv; run svc (1); } cache (8);
          hold kv (8) { observe hit = cached; }
          end;
        }
        run { horizon 100; }
    "#;
    assert_eq!(samples(&run(src), "hit"), [8.0]);
}

/// A hold on two pools gives one back early and keeps the other to its end:
/// vLLM's prefiller frees a finished request's slot while its blocks stay
/// leased for the decoder's read.
#[test]
fn a_hold_on_two_pools_releases_one_of_them() {
    let src = r#"
        pool slots { cap 1; }
        pool kv { cap 100; }
        stage svc : delay;
        workload { arrive batch(2); }
        session {
          run svc (serial * 0.5);
          set t0 = now;
          hold slots (1), kv (10) {
            observe got_slot = now;
            run svc (1);
            release slots;
            run svc (4);
            observe kv_used = used(kv);
          }
          end;
        }
        run { horizon 100; }
    "#;
    let r = run(src);
    assert_eq!(
        samples(&r, "got_slot"),
        [0.0, 1.0],
        "the slot is free at t = 1"
    );
    assert_eq!(
        samples(&r, "kv_used"),
        [20.0, 10.0],
        "both held kv at t = 5"
    );
}

/// `load` counts the loaded tokens as computed: the hold's position moves,
/// so the cache clause keeps them, as it keeps what a `growing` run
/// computed.
#[test]
fn load_advances_the_computed_position() {
    let src = r#"
        pool kv { cap 100; }
        stage svc : delay;
        workload { arrive batch(1); }
        session {
          hold kv (40) { run svc (1); load kv (30); } cache (40);
          hold kv (40) { observe hit = cached; }
          end;
        }
        run { horizon 100; }
    "#;
    // nothing was computed before the load: 30 of the 40 are cached
    assert_eq!(samples(&run(src), "hit"), [30.0]);
}

/// A load past the allocation is a program error, said at the statement.
#[test]
fn load_must_fit_the_allocation() {
    let src = r#"
        pool kv { cap 100; }
        stage svc : delay;
        workload { arrive batch(1); }
        session { hold kv (10) { load kv (11); } end; }
        run { horizon 10; }
    "#;
    let msg = run_source(src, &Overrides::default(), None).unwrap_err();
    assert!(msg.contains("must fit the allocation"), "{msg}");
}

/// `release` and `load` act on an enclosing hold; outside one they do not
/// link.
#[test]
fn release_and_load_need_an_enclosing_hold() {
    for stmt in ["release kv;", "load kv (1);", "hold q (1) { release kv; }"] {
        let src = format!(
            "pool kv {{ cap 10; }} pool q {{ cap 10; }} stage svc : delay;
             workload {{ arrive batch(1); }}
             session {{ {stmt} end; }} run {{ horizon 10; }}"
        );
        let e = check_source(&src, &Overrides::default())
            .err()
            .unwrap_or_else(|| panic!("`{stmt}` linked"));
        assert!(e.contains("outside a hold of `kv`"), "{stmt}: {e}");
    }
}

/// The transfer: the source's lease outlives its scope, the KV is in both
/// pools during the link run and in the destination alone after it. The source's next session (queued at
/// 0.5) is admitted when the transfer ends (t = 2), the destination's
/// (queued at 1.5) when the decode ends (t = 3).
#[test]
fn a_transfer_overlaps_the_two_pools_for_the_link_run() {
    let src = r#"
        pool memP { cap 10; }
        pool memD { cap 10; }
        stage prefill : delay;
        stage link : delay;
        stage decode : delay;
        stage gate : delay;
        workload { arrive batch(3); init { set kind = serial; } }
        session {
          branch (kind == 0) {
            hold memP (10) { prefill (1); } lease memP (inf);
            hold memD (10) {
              transfer (1) from memP to memD (10);
              observe p_holders = holders(memP);
              observe d_used = used(memD);
              decode (1);
            }
          }
          branch (kind == 1) { run gate (0.5); hold memP (10) { observe p_admitted = now; } }
          branch (kind == 2) { run gate (1.5); hold memD (10) { observe d_admitted = now; } }
          end;
        }
        run { horizon 100; }
    "#;
    let r = run(src);
    // the source is free the moment the link run ends (the waiting session
    // holds it now, alone), the destination is held on
    assert_eq!(samples(&r, "p_holders"), [1.0]);
    assert_eq!(samples(&r, "d_used"), [10.0]);
    assert_eq!(samples(&r, "p_admitted"), [2.0]);
    assert_eq!(samples(&r, "d_admitted"), [3.0]);
}

/// `transfer (w) from P to Q (n)` is `run link (w); load Q (n); release P;`.
#[test]
fn transfer_from_to_is_sugar_for_three_statements() {
    let a = "pool memP { cap 10; } pool memD { cap 10; } stage link : delay;
             workload { arrive batch(1); }
             session { hold memP (10) { hold memD (10) { transfer (1) from memP to memD (9); } } end; }
             run { horizon 10; }";
    let b = "pool memP { cap 10; } pool memD { cap 10; } stage link : delay;
             workload { arrive batch(1); }
             session { hold memP (10) { hold memD (10) { run link (1); load memD (9); release memP; } } end; }
             run { horizon 10; }";
    let ir = |s: &str| {
        seq::compile_source(s, &Overrides::default())
            .unwrap()
            .to_json()
    };
    assert_eq!(ir(a), ir(b));
}

/// The coupling a store-and-forward link cannot state: while the decoder has
/// no room, finished prefills sit in the prefiller's memory (leased), and
/// the prefiller stops admitting. With the lecture's shape (the prefiller
/// holds through the transfer, then the session queues for the decoder)
/// the prefiller keeps working.
#[test]
fn decode_pressure_backs_into_the_prefill_pool() {
    let nixl = r#"
        pool memP { cap 20; }
        pool memD { cap 10; }
        stage prefill : delay;
        stage link : delay;
        stage decode : delay;
        stage gate : delay;
        workload { arrive batch(6); }
        session {
          run gate (serial * 0.01);
          hold memP (10) { prefill (0.1); observe prefilled = serial; } lease memP (inf);
          hold memD (10) {
            transfer (0.1) from memP to memD (10);
            decode (10);
          }
          end;
        }
        run { horizon 15; }
    "#;
    let store_and_forward = r#"
        pool memP { cap 20; }
        pool memD { cap 10; }
        stage prefill : delay;
        stage link : delay;
        stage decode : delay;
        stage gate : delay;
        workload { arrive batch(6); }
        session {
          run gate (serial * 0.01);
          hold memP (10) { prefill (0.1); observe prefilled = serial; run link (0.1); }
          hold memD (10) { decode (10); }
          end;
        }
        run { horizon 15; }
    "#;
    // one decode of 10 s fits the decoder; by t = 15 one has finished and a
    // second is running. NIXL: the prefiller's two slots are taken by the
    // request decoding (its lease ended) ... no: by the requests waiting for
    // the decoder with their blocks leased, so only what the decoder drained
    // got prefilled.
    let r = run(nixl);
    let n = samples(&r, "prefilled").len();
    let s = run(store_and_forward);
    let m = samples(&s, "prefilled").len();
    assert_eq!(m, 6, "store-and-forward prefills everything at once");
    assert!(
        n < m,
        "NIXL: {n} prefilled, the rest wait for the decoder's memory"
    );
}

/// A hold re-executed after a preemption reaches its `release` again and
/// finds nothing to give back; the run neither frees units twice nor stops.
#[test]
fn a_re_executed_hold_releases_nothing_twice() {
    let src = r#"
        pool memP { cap 100; }
        pool memD { cap 20; block 10; preempt lifo; }
        stage link : delay;
        stage engine : step { budget 100; cost 1; memory memD; }
        workload { arrive batch(1); }
        session {
          hold memP (10) {
            hold memD (10) {
              transfer (1) from memP to memD (10);
              observe p_free = free(memP);
              run engine decode (25) growing memD;
            }
          }
          end;
        }
        run { horizon 30; }
    "#;
    let r = run(src);
    let free = samples(&r, "p_free");
    assert!(free.len() > 1, "the hold was re-executed: {free:?}");
    assert!(free.iter().all(|&f| f == 100.0), "{free:?}");
    assert!(r.pool("memD").unwrap().preemptions > 0);
}

/// A step stage that serves several queues tries them in the order their
/// pools are declared, and the first head that does not fit stops the
/// step's admissions. `examples/pd-disaggregation/llmd_nixl_pull.seq` relies on it: the decoder's
/// requests whose KV has arrived (its `reqsD` queue) are declared before
/// the new ones (`kvD`), as vLLM serves `skipped_waiting` before `waiting`.
#[test]
fn an_engine_serves_its_queues_in_declaration_order() {
    let program = |first: &str, second: &str| {
        format!(
            r#"
            pool {first} {{ cap 1; admit via engine; }}
            pool {second} {{ cap 1; admit via engine; }}
            stage engine : step {{ budget 1000; cost 1; }}
            stage gate : delay;
            workload {{ arrive batch(3); }}
            session {{
              run gate (serial);
              branch (serial == 0) {{ hold a (1) {{ run engine decode (50); }} }}
              branch (serial == 1) {{ hold a (1) {{ run engine decode (1); }} }}
              branch (serial == 2) {{ hold b (1) {{ observe b_admitted = now; run engine decode (1); }} }}
              end;
            }}
            run {{ horizon 200; }}
            "#
        )
    };
    // `a` first: its head (session 1, no room until 50) blocks `b`'s
    let r = run(&program("a", "b"));
    assert_eq!(samples(&r, "b_admitted"), [50.0], "{}", r.text());
    // `b` first: session 2 is admitted by the step that starts as it queues
    let r = run(&program("b", "a"));
    assert_eq!(samples(&r, "b_admitted"), [2.0], "{}", r.text());
}

/// The whole program under decoder memory pressure: preemptions on the
/// decoder, whose requests were admitted twice (blocks, then a slot), pick
/// their victim by the residents' order and never a request the step has
/// already served. Before, the victim was the last holder of the pool by
/// its block allocation, which could be a request served earlier in the
/// same step, and the step then looked up a job it had removed.
#[test]
fn the_pd_program_survives_decoder_memory_pressure() {
    let src = std::fs::read_to_string(seq::program_path("llmd_nixl_pull")).unwrap();
    let ov = Overrides {
        lets: vec![(
            "blocksD".into(),
            seq::frontend::parser::parse_expr("1200").unwrap(),
        )],
        horizon: Some(400.0),
        warmup: Some(50.0),
        ..Default::default()
    };
    let path = seq::program_path("llmd_nixl_pull");
    let r = run_source(&src, &ov, path.parent()).unwrap();
    assert!(r.pool("kvD").unwrap().preemptions > 0, "{}", r.text());
    assert!(
        r.observe("lease").unwrap().samples.len() > 100,
        "{}",
        r.text()
    );
}

/// `release` and `load` name the pool as the enclosing hold wrote it,
/// index included: `hold q[0] { load q[1] (1); }` would look for a hold the
/// session does not have.
#[test]
fn release_and_load_name_the_pool_as_the_hold_does() {
    let program = |body: &str| {
        format!(
            "pool q[2] {{ cap 10; }} stage svc : delay;
             workload {{ arrive batch(1); init {{ set j = 0; }} }}
             session {{ {body} end; }} run {{ horizon 10; }}"
        )
    };
    let e = check_source(
        &program("hold q[0] (1) { load q[1] (1); }"),
        &Overrides::default(),
    )
    .expect_err("linked");
    assert!(e.contains("index included"), "{e}");
    let e = check_source(
        &program("hold q[j] (1) { release q[0]; }"),
        &Overrides::default(),
    )
    .expect_err("linked");
    assert!(e.contains("index included"), "{e}");
    check_source(
        &program("hold q[j] (1) { run svc (1); release q[j]; }"),
        &Overrides::default(),
    )
    .expect("links");
}

/// `preempt lifo` on an engine's memory with nobody resident: the grower
/// waits for room instead of preempting a holder that is away from the
/// engine (leased, parked). A holds 10 and B holds 10 of 20 at a delay
/// stage; A's growth at t = 1 finds no room and no resident, and goes
/// through when B releases at 5.
#[test]
fn a_grow_with_nobody_to_preempt_waits() {
    let src = r#"
        pool kv { cap 20; preempt lifo; }
        stage engine : step { budget 100; cost 1; memory kv; }
        stage svc : delay;
        workload { arrive batch(2); }
        session {
          hold kv (10) {
            branch (serial == 0) { run svc (1); grow kv (10); observe grew = now; run svc (1); }
            else { run svc (5); }
          }
          end;
        }
        run { horizon 50; }
    "#;
    let r = run(src);
    assert_eq!(samples(&r, "grew"), [5.0], "{}", r.text());
    assert_eq!(r.pool("kv").unwrap().preemptions, 0, "{}", r.text());
}

/// `lease P (t)`: the allocation outlives the scope. Untaken, it ends at
/// the bound and keeps its cache; session 1 (queued at 0.5 for the whole
/// pool) is admitted at 3, when session 0's lease of 2 s ends, not at 1.
#[test]
fn an_untaken_lease_ends_at_its_bound_and_keeps_its_cache() {
    let src = r#"
        pool kv { cap 10; }
        stage svc : delay;
        stage gate : delay;
        workload { arrive batch(2); }
        session {
          branch (serial == 0) {
            hold kv (10) { run svc (1); } cache (10) lease kv (2);
            observe leased = used(kv);
            run svc (5);
            hold kv (10) { observe hit = cached; }
          } else {
            run gate (0.5);
            hold kv (10) { observe admitted = now; }
          }
          end;
        }
        run { horizon 100; }
    "#;
    let r = run(src);
    assert_eq!(
        samples(&r, "leased"),
        [10.0],
        "still allocated after the scope"
    );
    assert_eq!(samples(&r, "admitted"), [3.0], "{}", r.text());
    // session 1 held the whole pool at 3, which evicted session 0's cache
    assert_eq!(samples(&r, "hit"), [0.0]);
}

/// A lease the session never takes ends with the session, and `cache`
/// applies then: the next session finds the pool free at once and the
/// cache still there for its owner would have been.
#[test]
fn a_lease_ends_with_the_session() {
    let src = r#"
        pool kv { cap 10; }
        stage svc : delay;
        workload { arrive batch(2); }
        session {
          branch (serial == 0) {
            hold kv (10) { run svc (1); } cache (10) lease kv (inf);
            run svc (1);
          } else {
            run svc (0.5);
            hold kv (10) { observe admitted = now; }
          }
          end;
        }
        run { horizon 100; }
    "#;
    let r = run(src);
    assert_eq!(samples(&r, "admitted"), [2.0], "{}", r.text());
}

/// A lease is never a preemption victim: with `preempt lifo` on a pool
/// that is no engine's memory, the last holder in a scope is preempted,
/// not the session whose lease is the most recent allocation.
#[test]
fn a_lease_is_not_a_preemption_victim() {
    let src = r#"
        pool kv { cap 20; preempt lifo; }
        stage svc : delay;
        workload { arrive batch(2); }
        session {
          branch (serial == 0) {
            hold kv (5) { run svc (1); grow kv (10); observe grew = now; run svc (1); }
          } else {
            run svc (0.5);
            hold kv (10) { run svc (0.1); } lease kv (inf);
            run svc (2);
            release kv;
            observe released = now;
          }
          end;
        }
        run { horizon 100; }
    "#;
    let r = run(src);
    // session 0 needs 15 of 20 at t = 1 and again at 2; session 1's lease
    // holds 10 and is not the victim: the only holder in a scope is the
    // grower, which preempts itself twice, re-enters at once (5 fit) and
    // grows at 3, after the lease was released at 2.6
    assert_eq!(r.pool("kv").unwrap().preemptions, 2, "{}", r.text());
    assert_eq!(samples(&r, "released"), [2.6]);
    assert_eq!(samples(&r, "grew"), [3.0], "{}", r.text());
}

/// `lease` names one of the hold's pools, as the hold writes it.
#[test]
fn a_lease_names_a_pool_of_the_hold() {
    let bad = "pool a { cap 1; } pool b { cap 1; } stage svc : delay;
               workload { arrive batch(1); }
               session { hold a (1) { run svc (1); } lease b (1); end; } run { horizon 10; }";
    let e = check_source(bad, &Overrides::default()).expect_err("linked");
    assert!(e.contains("does not take that pool"), "{e}");
    // and a `release` of a leased pool links outside any hold of it
    let ok = "pool a { cap 1; } stage svc : delay;
              workload { arrive batch(1); }
              session { hold a (1) { run svc (1); } lease a (1); run svc (1); release a; end; } run { horizon 10; }";
    check_source(ok, &Overrides::default()).expect("links");
}
