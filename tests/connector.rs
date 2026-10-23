//! An engine's KV connector (`docs/design/scheduler-worker.md`, step 4):
//! `connector C { per_token (r); load (n); save (n); }` copies a request's
//! KV in from the L2 queue `C` at its admission and out to it at its end,
//! at `C`'s bandwidth, as vLLM's `KVConnector` does for LMCache (#555).

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
        "`connector` twice",
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
                       cache kv (request.input);
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
