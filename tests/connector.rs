//! An engine's KV connector (`docs/design/scheduler-worker.md`, step 4):
//! `connector C { per_token (r); load (n); save (n); }` copies a request's
//! KV in from the L2 queue `C` at its admission and out to it at its end,
//! at `C`'s bandwidth, as vLLM's `KVConnector` does for LMCache (#555). An
//! engine's several connectors are vLLM's `MultiConnector`: a request loads
//! from the first that has tokens for it (`docs/design/multi-connector.md`,
//! #599).

mod common;

fn run(src: &str, workload: &str, horizon: f64) -> serq::Report {
    common::run(
        &common::main_source(src),
        workload,
        &common::horizon(horizon),
    )
    .unwrap_or_else(|e| panic!("{e}\n{src}\n{workload}"))
}

/// One request, one turn: the workload of the tests that refuse a program.
const ONE: &str = "workload { arrive batch(1); }";

fn refused(src: &str, needle: &str) {
    let e = match common::compile(&common::main_source(src), ONE, &common::horizon(100.0)) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("accepted:\n{src}"),
    };
    assert!(e.contains(needle), "{src}\n  {e}");
}

fn samples(r: &serq::Report, name: &str) -> Vec<f64> {
    r.observe(name).unwrap().samples.clone()
}

const CACHE: &str = "queue lmc : l2 { pool kv { cap 1000; } serve ps(1); }";

/// Engine `e` with `connector` and `taking` as written; one iteration a
/// second, whatever it computes.
fn engine(connector: &str, taking: &str) -> String {
    format!(
        "{CACHE}
         device gpu {{ kv cap 1000; }}
         pool kv on gpu {{ }}
         engine e on gpu {{
           tokens cap inf;
           {connector}
           scheduler {{ advance running; admit waiting while (running.preempted == 0) taking {taking}; }}
           worker {{ forward (1); }}
         }}"
    )
}

/// The first turn's 100 tokens are prefilled in an iteration and saved at
/// 0.01 s a token, 1 s, before the request ends. The second turn loads 50
/// of them (0.5 s) and prefills the other 50, its first token 1.5 s after
/// its admission; its save copies nothing, the L2 holding the 100 already.
/// The L2 keeps what was loaded from it.
#[test]
fn a_connector_saves_at_the_end_and_loads_at_the_admission() {
    let r = run(
        &format!(
            "{}
             server {{
               e (100, 1);
               observe loaded = e.loaded;
               observe to_first = e.first - e.admitted;
               observe save_time = now - e.first;
               observe held = cachedin(lmc.kv);
             }}",
            engine(
                "connector lmc {
                   per_token (0.01);
                   load (max(0, min(cachedin(lmc.kv), 50) - request.hit));
                   save (request.input);
                 }",
                "kv (request.known)"
            )
        ),
        "workload { arrive batch(1); turn { } session { turn; tool (50); turn; end; } }",
        1000.0,
    );
    assert_eq!(samples(&r, "loaded"), [0.0, 50.0]);
    assert_eq!(samples(&r, "e.loaded"), [0.0, 50.0]);
    assert_eq!(samples(&r, "e.prefill"), [100.0, 50.0]);
    assert_eq!(samples(&r, "to_first"), [1.0, 1.5]);
    assert_eq!(samples(&r, "save_time"), [1.0, 0.0]);
    assert_eq!(samples(&r, "held"), [100.0, 100.0]);
}

/// `request.preempted` is 1 at the admissions after a preemption, which a
/// connector reads as LMCache's does (`lmcache_mp_connector.py:758`): two
/// requests that outgrow 48 tokens of KV, one preempted once, load 8 at
/// their first admissions and 0 at the one after the preemption.
#[test]
fn a_connector_reads_whether_the_request_was_preempted() {
    let r = run(
        &format!(
            "{CACHE}
             device gpu {{ kv cap 48; }}
             pool kv on gpu {{ block 16; evict lru; preempt lifo; }}
             engine e on gpu {{
               tokens cap 64;
               connector lmc {{ per_token (0.01); load (request.preempted ? 0 : 8); }}
               scheduler {{ advance running; admit waiting while (running.preempted == 0) taking kv (request.known); }}
               worker {{ forward (1); }}
             }}
             server {{ e (16, 20); }}"
        ),
        "workload { arrive batch(2); }",
        1000.0,
    );
    assert_eq!(r.pool("kv").unwrap().preemptions, 1, "{}", r.text());
    assert_eq!(samples(&r, "e.loaded"), [8.0, 8.0, 0.0]);
}

/// The bug as it was found (#522 review): in `vllm_lmcache.sq` at a low
/// rate a load of 16 tokens (8e-5 s) took 532 s. The cache is declared
/// before the engine, and the copy starts when the engine's iteration admits
/// the request, after the instant's one pass in stage order had gone past
/// the cache's stage; nothing rescheduled it until the next session
/// arrived. The pass now starts the iterations first and reschedules the
/// `ps` stages after them.
#[test]
fn an_l2_load_at_a_low_rate_is_not_left_waiting() {
    let path = serq::program_path("vllm_lmcache");
    let mut o = common::horizon(6000.0);
    o.set("Lambda", "0.005").unwrap();
    let workload = serq::paired_workload(&path).expect("vllm_lmcache's workload");
    let r = serq::load_paired(&path, &workload, &o)
        .and_then(|p| serq::run_ir(&p, workload.parent()))
        .unwrap_or_else(|e| panic!("{e}"));
    let ttft = &r.client("ttft").unwrap().samples;
    let worst = ttft.iter().cloned().fold(0.0, f64::max);
    assert!(worst < 1.0, "the slowest first token took {worst} s");
}

/// A connector names an L2 queue, the time a token's copy takes and at
/// least one copy; its load says what `request.loaded` is and computes into
/// the engine's memory; its copy time is read where `cached` is the L2's.
#[test]
fn a_connector_says_what_it_copies() {
    let taking = "kv (request.known)";
    let with = |c: &str| engine(c, taking) + " server { e (10, 1); }";
    refused(
        &with("connector kv { per_token (1); load (1); }"),
        "`connector kv`: `kv` is not a queue that plays `l2` declared above",
    );
    refused(
        &with("connector lmc { load (1); }"),
        "a connector needs `per_token (r);`",
    );
    refused(
        &with("connector lmc { per_token (1); }"),
        "a connector needs `load (n);`, `save (n);` or both",
    );
    refused(
        &with("connector lmc { per_token (cached); save (1); }"),
        "`per_token` is read as a copy starts, where `cached` would be the L2 entry's",
    );
    refused(
        &with("connector lmc { per_token (1); load (request.loaded); }"),
        "a connector's `load` says what `request.loaded` is",
    );
    refused(
        &with(
            "connector lmc { per_token (1); load (1); } connector lmc { per_token (1); save (1); }",
        ),
        "`connector lmc` twice: one connector names its queue, with its `load` and its `save`",
    );
    refused(
        &(engine("connector lmc { per_token (1); load (1); }", "reqs (1)")
            .replace("tokens cap inf;", "tokens cap inf; reqs cap 4;")
            + " pool reqs on e { } server { e (10, 1); }"),
        "loads into the engine's memory",
    );
    // `request.loaded` and `request.preempted` are the admission's
    refused(
        &(engine("connector lmc { per_token (1); load (1); }", taking)
            .replace("forward (1);", "forward (1 + request.loaded);")
            + " server { e (10, 1); }"),
        "`request.loaded`",
    );
    // the caller reads `e.loaded` of an engine that has a connector
    refused(
        &(engine("", taking) + " server { e (10, 1); observe l = e.loaded; }"),
        "e.loaded",
    );
}

/// An L2 is one pool and a `serve`; a request's copies through it are the
/// engine's connector, and std/kvt's statements say so.
#[test]
fn an_l2_is_one_pool_and_a_serve() {
    let with = |cache: &str| {
        engine(
            "connector lmc { per_token (1); load (1); }",
            "kv (request.known)",
        )
        .replace(CACHE, cache)
            + " server { e (10, 1); }"
    };
    refused(
        &with("queue lmc : l2 { pool kv { cap 10; } }"),
        "plays `l2`: it is one pool and a `serve`",
    );
    refused(
        &with("queue lmc : l2 { pool kv { cap 10; } pool kw { cap 10; } serve ps(1); }"),
        "plays `l2`: it is one pool and a `serve`",
    );
    refused(
        &with("queue lmc[2] : l2 { pool kv { cap 10; } serve ps(1); }"),
        "plays `l2` and is a family",
    );
    for f in ["l2_load", "l2_store"] {
        refused(
            &format!(
                "use \"std/kvt\"; {CACHE} pool kv {{ cap 1000; }}
                 server {{ hold kv (cost(kv, 50)) {{ kvt.{f} (lmc, kv, 20, per_token = 0.5); }} }}"
            ),
            &format!("`kvt.{f}` is an engine's connector now"),
        );
    }
}

/// A connector's `load` is read at the admission, as `taking` is: it may
/// read the registers of the engine that admits the request, not another
/// engine's, which the order of the declarations would answer (#367; #592
/// review).
#[test]
fn a_connector_reads_its_own_engine_s_registers() {
    let src = format!(
        "{CACHE}
         device tpu {{ }}
         engine a on tpu {{
           tokens cap 8;
           state go = 0;
           scheduler {{ advance running; admit waiting; set go = 1; }}
           worker {{ forward (1); }}
         }}
         device gpu {{ kv cap 1000; }}
         pool kv on gpu {{ block 16; }}
         engine e on gpu {{
           reqs cap 4;
           tokens cap 64;
           connector lmc {{ per_token (1e-6); load (go * 16); }}
           scheduler {{ advance running; admit waiting taking reqs (1), kv (request.known); }}
           worker {{ forward (1); }}
         }}
         pool reqs on e {{ }}
         server {{ a (2, 1); e (40, 2); }}"
    );
    refused(
        &src,
        "`go` is engine `a`'s register, and engine `e`'s admission would read it",
    );
}

/// `cachedin(P)` reads `P` by the chain the session cached along there, not
/// by a hold that does not hold `P` (#595, ported from `kvt.l2_store`'s
/// test). The connector saves along the request's own chain while its
/// admission holds `kv` with a `prefix`: outside, `cachedin(lmc.kv)` read 0
/// of the L2's entry and `cachedin(kv)` 0 of `kv`'s, cached along the
/// prefix's chain; inside, the load read 0. A system prompt of length 0 hid
/// all three: the two chains are one then. The L2 holds the 100 tokens
/// saved, `kv` the 200 cached, 192 in whole blocks of 16, and the second
/// turn loads 10 of the L2's.
#[test]
fn cachedin_reads_each_pool_by_the_chain_it_was_cached_along() {
    for system in [16, 0] {
        let r = run(
            &format!(
                "{CACHE}
                 device gpu {{ kv cap 1000; }}
                 pool kv on gpu {{ block 16; }}
                 engine e on gpu {{
                   tokens cap inf;
                   connector lmc {{ per_token (0.01); load (min(cachedin(lmc.kv), 10)); save (100); }}
                   scheduler {{
                     advance running;
                     admit waiting taking kv (request.known) prefix (0: {system}, serial: inf)
                       cache (request.input);
                   }}
                   worker {{ forward (1); }}
                 }}
                 server {{
                   observe l2_outside = cachedin(lmc.kv);
                   observe kv_outside = cachedin(kv);
                   e (200, 1);
                 }}"
            ),
            "workload { arrive batch(1); turn { } session { turn; turn; end; } }",
            100.0,
        );
        assert_eq!(
            samples(&r, "l2_outside"),
            [0.0, 100.0],
            "{system}\n{}",
            r.text()
        );
        assert_eq!(samples(&r, "kv_outside"), [0.0, 192.0], "{system}");
        assert_eq!(samples(&r, "e.loaded"), [0.0, 10.0], "{system}");
    }
}

/// Two L2s, `a` and `b`, and an engine that asks them in their order, one
/// iteration a second (multi_connector.py:429-448). The first request (100
/// tokens) has 20 at `a`, which loads them at 0.01 s a token, 0.2 s, and is
/// asked first: `b`'s 30 are passed over. It prefills the other 80 in an
/// iteration, its first token 1.2 s after its admission. The second (60)
/// has none at `a`, and loads `b`'s 30 at 0.02 s a token, 0.6 s, then
/// prefills 30: 1.6 s. `b` saves, `a` does not: `b` holds the first's 100
/// after it ends. Written the other way round, `b`, which always has 30, is
/// asked first and loads both.
#[test]
fn an_engine_loads_from_its_first_connector_that_has_tokens() {
    let src = |connectors: &str| {
        format!(
            "queue a : l2 {{ pool kv {{ cap 1000; }} serve ps(1); }}
             queue b : l2 {{ pool kv {{ cap 1000; }} serve ps(1); }}
             device gpu {{ kv cap 1000; }}
             pool kv on gpu {{ }}
             engine e on gpu {{
               tokens cap inf;
               {connectors}
               scheduler {{ advance running; admit waiting while (running.preempted == 0) taking kv (request.known); }}
               worker {{ forward (1); }}
             }}
             server {{
               e (100, 1);
               observe first_to_first = e.first - e.admitted;
               observe held = cachedin(b.kv);
               e (60, 1);
               observe second_to_first = e.first - e.admitted;
             }}"
        )
    };
    let a = "connector a { per_token (0.01); load (request.input == 100 ? 20 : 0); }";
    let b = "connector b { per_token (0.02); load (30); save (request.input); }";
    let w = "workload { arrive batch(1); }";
    // the times to the first token, to the float's last digit
    let firsts = |r: &serq::Report| -> Vec<f64> {
        ["first_to_first", "second_to_first"]
            .map(|o| (samples(r, o)[0] * 1e9).round() / 1e9)
            .to_vec()
    };
    let r = run(&src(&format!("{a} {b}")), w, 1000.0);
    assert_eq!(samples(&r, "e.loaded"), [20.0, 30.0]);
    assert_eq!(samples(&r, "e.prefill"), [80.0, 30.0]);
    assert_eq!(firsts(&r), [1.2, 1.6]);
    assert_eq!(samples(&r, "held"), [100.0]);
    assert_eq!(r.stage("a").map(|s| s.completed), Some(1));
    let r = run(&src(&format!("{b} {a}")), w, 1000.0);
    assert_eq!(samples(&r, "e.loaded"), [30.0, 30.0]);
    assert_eq!(firsts(&r), [1.6, 1.6]);
    assert_eq!(r.stage("a").map(|s| s.completed), Some(0));
}

/// An engine's connectors name a queue each, one connector holding its
/// `load` and its `save`; and one of them saves: a request runs one
/// transfer at a time, so two saves would run one after the other, where
/// vLLM's run at once (multi_connector.py:334-346, 525-554).
#[test]
fn an_engine_s_connectors_name_a_queue_once_and_save_once() {
    let two = "queue lmc2 : l2 { pool kv { cap 1000; } serve ps(1); }";
    let with = |c: &str| {
        engine(c, "kv (request.known)").replace(CACHE, &format!("{CACHE} {two}"))
            + " server { e (10, 1); }"
    };
    refused(
        &with(
            "connector lmc2 { per_token (1); load (1); } connector lmc { per_token (1); load (1); } \
             connector lmc2 { per_token (1); save (1); }",
        ),
        "`connector lmc2` twice: one connector names its queue",
    );
    refused(
        &with(
            "connector lmc { per_token (1); save (1); } connector lmc2 { per_token (1); load (1); \
             save (1); }",
        ),
        "`connector lmc2` saves, and so does `connector lmc`: a finished request runs one save",
    );
    // one saves, both load: it links
    let ok = with(
        "connector lmc { per_token (1); load (1); save (1); } connector lmc2 { per_token (1); \
         load (1); }",
    );
    common::compile(&common::main_source(&ok), ONE, &common::horizon(10.0))
        .unwrap_or_else(|e| panic!("{e}\n{ok}"));
}

/// A P/D pair with a shared L2, as `llmd_nixl_lmcache.sq`: the decoder asks
/// its prefiller's connector first, then the L2's. The first turn (10
/// tokens) is remote: the prefiller's iteration ends at 1, the decoder
/// `D[0]` reads the 10 over the NICs (the wait of 1, then 10 tokens at 10
/// a second: 3), computes its last token again (4) and decodes its second
/// (5), then saves the 12 it has at 0.01 s a token: the turn takes 5.12 and
/// the L2 holds 12. The second (20) is local to `D[1]`, whose memory holds
/// nothing: its entry took its KV from no prefiller (`D.from` -1), so the
/// L2 is asked, and loads the 12 it holds (0.12), the decoder prefills the
/// other 8 (1.12) and decodes (2.12), and saves the 10 the L2 lacks of its
/// 22 (0.1): 2.22. One copy over the NICs, three at the L2.
#[test]
fn a_decoder_reads_a_remote_request_from_its_source_and_a_local_one_from_the_l2() {
    let src = r#"
        queue gw : gateway {
          route {
            set t0 = now;
            if (prompt == 10) { P.prefill (prompt); D[0].decode (prompt) from P; } else { D[1].decode (prompt); }
            observe took = now - t0;
            observe held = cachedin(lmc.kv);
          }
        }
        queue lmc : l2 { pool kv { cap 1000; } serve ps(1); }
        queue P : prefill {
          device gpu { kv cap 100; }
          engine on gpu {
            reqs cap 4;
            tokens cap inf;
            scheduler { advance running; admit waiting while (running.preempted == 0) taking reqs (1), kv (request.known) cache (request.input); }
            worker { forward (1); }
          }
          pool reqs on P { }
          pool kv on gpu { }
          nic ps(10);
          prefill (p) { P (p, 1); }
        }
        queue D[2] : agg, decode {
          device gpu { kv cap 100; }
          engine on gpu {
            reqs cap 4;
            tokens cap inf;
            connector P { load (request.input - request.hit); }
            connector lmc {
              per_token (0.01);
              load (max(0, min(cachedin(lmc.kv), request.known - 1) - request.hit));
              save (request.input + request.output);
            }
            scheduler { advance running; admit waiting while (running.preempted == 0) taking kv (request.known), reqs (request.loaded > 0 ? 0 : 1) cache (request.input); }
            worker { forward (1); }
          }
          pool reqs on D { }
          pool kv on D.gpu { }
          nic ps(10);
          decode (p) { D (p, 2); }
          decode (p) from P { D (p, 2); }
        }
        use "std/kvt"; use "std/share"; kvt.nixl_pull (P, D, latency = 1, share = share.maxmin);
        server (prompt) { gw.route(); }
    "#;
    let r = run(
        src,
        "workload { arrive batch(1); init { set prompt = 10; } send (prompt); \
         session { turn; set prompt = 20; turn; end; } }",
        1000.0,
    );
    assert_eq!(samples(&r, "D.loaded"), [10.0, 12.0]);
    assert_eq!(samples(&r, "D.prefill"), [1.0, 8.0]);
    assert_eq!(samples(&r, "held"), [12.0, 22.0]);
    let took = samples(&r, "took");
    assert!(
        (took[0] - 5.12).abs() < 1e-9 && (took[1] - 2.22).abs() < 1e-9,
        "{took:?}"
    );
    let done = |name: &str| -> u64 { r.stages_named(name).iter().map(|s| s.completed).sum() };
    assert_eq!(done("D.nic"), 1);
    assert_eq!(done("lmc"), 3);
}

/// The bug as found (#599): in `llmd_nixl_pull.sq` with `blocksD` 1000 the
/// program's `send_held` (`D.load_done - t1`) of a decoder's request that
/// was preempted after its read ran to its admission again, where the read
/// had ended the send long before. The admission found the send gone and
/// called its load over then, though its load, `request.preempted ? 0 :
/// …`, asked for nothing. Here two prompts of 16 are prefilled by 1 and
/// read by 5.2 (the wait of 1, then 32 tokens at 10 a second); their decode
/// outgrows the decoder's 48 tokens and the later admitted is preempted,
/// admitted again at 25.2. Its load is still over at 5.2: a load of no
/// tokens is no load, and the first load that has tokens is the request's,
/// as for several connectors.
#[test]
fn a_preempted_request_that_loads_nothing_keeps_its_read_s_end() {
    let src = r#"
        queue gw : gateway {
          route {
            set t0 = now;
            P.prefill (prompt);
            D.decode (prompt) from P;
            observe loaded_at = D.load_done - t0;
          }
        }
        queue P : prefill {
          device gpu { kv cap 1000; }
          engine on gpu {
            reqs cap 4;
            tokens cap inf;
            scheduler { advance running; admit waiting while (running.preempted == 0) taking reqs (1), kv (request.known) cache (request.input); }
            worker { forward (1); }
          }
          pool reqs on P { }
          pool kv on gpu { }
          nic ps(10);
          prefill (p) { P (p, 1); }
        }
        queue D : agg, decode {
          device gpu { kv cap 48; }
          engine on gpu {
            reqs cap 4;
            tokens cap 64;
            connector P { load (request.preempted ? 0 : request.input - request.hit); }
            scheduler { advance running; admit waiting while (running.preempted == 0) taking kv (request.known), reqs (request.loaded > 0 ? 0 : 1) cache (request.input); }
            worker { forward (1); }
          }
          pool reqs on D { }
          pool kv on gpu { block 16; evict lru; preempt lifo; }
          nic ps(10);
          decode (p) { D (p, 20); }
          decode (p) from P { D (p, 20); }
        }
        use "std/kvt"; use "std/share"; kvt.nixl_pull (P, D, latency = 1, share = share.maxmin);
        server (prompt) { gw.route(); }
    "#;
    let r = run(
        src,
        "workload { arrive batch(2); init { set prompt = 16; } send (prompt); session { turn; end; } }",
        1000.0,
    );
    assert_eq!(r.pool("D.kv").unwrap().preemptions, 1, "{}", r.text());
    assert_eq!(samples(&r, "D.loaded"), [16.0, 16.0, 0.0]);
    let at = samples(&r, "loaded_at");
    assert!(at.iter().all(|&t| (t - 5.2).abs() < 1e-9), "{at:?}");
}
