//! std/kvt's L2: a queue that plays `l2` is one pool and a `serve`, and
//! `kvt.l2_load` and `kvt.l2_store` copy KV in and out of it at its
//! bandwidth, leaving what it holds there (#522).

mod common;

use serq::{Overrides, check_source, compile_source, run_source};

fn run(src: &str, options: &Overrides) -> serq::Report {
    run_source(&common::main_source(src), options, None).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn refused(src: &str, needle: &str) {
    let e = match check_source(&common::main_source(src), &common::horizon(100.0)) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("accepted:\n{src}"),
    };
    assert!(e.contains(needle), "{src}\n  {e}");
}

fn samples(r: &serq::Report, name: &str) -> Vec<f64> {
    r.observe(name).unwrap().samples.clone()
}

const CACHE: &str = "use \"std/kvt\"; queue lmc : l2 { pool kv { cap 1000; } serve ps(1); }";
const SESSION: &str = "workload { arrive batch(1); init { set K = 0; } turn { } session { turn; set K = 1; turn; end; } }";

/// The two copies are the kernel's statements: a load is a run at the
/// cache's bandwidth and a `load`, nothing released; a store is a hold on
/// the cache's pool that takes the entry it has, grows by the rest and
/// copies it, and caches the whole.
#[test]
fn the_l2_copies_are_the_kernel() {
    let kvt = format!(
        "{CACHE}
         pool kv {{ cap 1000; }}
         {SESSION}
         server {{
           hold kv (cost(kv, 50)) {{
             kvt.l2_load (lmc, kv, 20, per_token = 0.5);
             kvt.l2_store (kv, lmc, 40, per_token = 0.5);
           }}
         }}"
    );
    let flat = format!(
        "pool l2 {{ cap 1000; }}
         stage bw : ps(1);
         pool kv {{ cap 1000; }}
         {SESSION}
         server {{
           hold kv (cost(kv, 50)) {{
             run bw (cost(bw, 20 * 0.5));
             load kv (cost(kv, 20));
             hold l2 (cost(l2, cachedin(l2))) {{
               grow l2 (cost(l2, max(0, 40 - cached)));
               run bw (cost(bw, max(0, 40 - cached) * 0.5));
               load l2 (cost(l2, max(0, 40 - cached)));
             }} cache (cost(l2, 40));
           }}
         }}"
    );
    let compile = |src: &str| {
        compile_source(&common::main_source(src), &common::horizon(100.0))
            .unwrap_or_else(|e| panic!("{e}\n{src}"))
            .to_json()
    };
    let got = compile(&kvt)
        .replace("\"lmc.kv\"", "\"l2\"")
        .replace("\"lmc\"", "\"bw\"");
    assert_eq!(got, compile(&flat));
}

/// A store copies only what the cache does not hold yet: the second turn's
/// 300 tokens take 2 s past the first turn's 100, not 3, and the entry
/// holds 300 after it.
#[test]
fn an_l2_store_copies_what_the_cache_does_not_hold() {
    let r = run(
        &format!(
            "{CACHE}
             pool kv {{ cap 1000; }}
             {SESSION}
             server {{
               observe before = cachedin(lmc.kv);
               hold kv (cost(kv, 10)) {{
                 kvt.l2_store (kv, lmc, K == 0 ? 100 : 300, per_token = 0.01);
               }}
               observe after = cachedin(lmc.kv);
               observe done = now;
             }}"
        ),
        &common::horizon(100.0),
    );
    assert_eq!(samples(&r, "before"), vec![0.0, 100.0]);
    assert_eq!(samples(&r, "after"), vec![100.0, 300.0]);
    let done = samples(&r, "done");
    assert!(
        (done[0] - 1.0).abs() < 1e-9 && (done[1] - 3.0).abs() < 1e-9,
        "{done:?}"
    );
}

/// A load copies what the cache holds into the hold, which counts it as
/// computed, and the cache still holds it.
#[test]
fn an_l2_load_leaves_the_cache_its_kv() {
    let r = run(
        &format!(
            "{CACHE}
             pool kv {{ cap 1000; }}
             {SESSION}
             server {{
               branch (K == 0) {{
                 hold kv (cost(kv, 10)) {{ kvt.l2_store (kv, lmc, 100, per_token = 0.01); }}
               }} else {{
                 hold kv (cost(kv, 100)) {{
                   kvt.l2_load (lmc, kv, cachedin(lmc.kv), per_token = 0.01);
                 }}
               }}
               observe held = cachedin(lmc.kv);
               observe done = now;
             }}"
        ),
        &common::horizon(100.0),
    );
    assert_eq!(samples(&r, "held"), vec![100.0, 100.0]);
    let done = samples(&r, "done");
    assert!((done[1] - 2.0).abs() < 1e-9, "{done:?}");
}

/// An engine's request reads the cache's pool at its admission, which is
/// another queue's and refused for any other queue.
#[test]
fn an_engine_reads_the_cache_it_copies_from() {
    let engine = |pool: &str| {
        format!(
            "{CACHE}
             queue other : link {{ pool kv {{ cap 10; }} serve ps(1); }}
             device gpu {{ kv cap 1000; }}
             pool kv on gpu {{ }}
             engine e on gpu {{
               tokens cap inf;
               request (p) {{
                 at admission {{ got = min(cachedin({pool}), p); }}
                 prefill (p) growing kv;
               }}
               schedule {{ advance running; admit waiting while (running.preempted == 0) taking kv (got + 1); }}
               execute (1);
             }}
             {SESSION}
             server {{ e (5); }}"
        )
    };
    check_source(
        &common::main_source(&engine("lmc.kv")),
        &common::horizon(10.0),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    refused(&engine("other.kv"), "which is not a pool or stage of `e`");
}

/// An L2 is one pool and a `serve`, and std/kvt's statements say what
/// they copy.
#[test]
fn an_l2_and_its_copies_say_what_they_are() {
    let with = |cache: &str, body: &str| {
        format!(
            "use \"std/kvt\"; {cache}
             pool kv {{ cap 1000; }}
             {SESSION}
             server {{ hold kv (cost(kv, 50)) {{ {body} }} }}"
        )
    };
    let load = "kvt.l2_load (lmc, kv, 20, per_token = 0.5);";
    refused(
        &with("queue lmc : l2 { pool kv { cap 10; } }", load),
        "plays `l2`: it is one pool and a `serve`",
    );
    refused(
        &with(
            "queue lmc : l2 { pool kv { cap 10; } pool kw { cap 10; } serve ps(1); }",
            load,
        ),
        "plays `l2`: it is one pool and a `serve`",
    );
    refused(
        &with(
            "queue lmc[2] : l2 { pool kv { cap 10; } serve ps(1); }",
            load,
        ),
        "plays `l2` and is a family",
    );
    let cache = "queue lmc : l2 { pool kv { cap 1000; } serve ps(1); }";
    refused(
        &with(cache, "kvt.l2_load (kv, kv, 20, per_token = 0.5);"),
        "`kvt.l2_load`: `kv` is not a queue that plays `l2` declared above",
    );
    refused(
        &with(cache, "kvt.l2_load (lmc, kv, 20);"),
        "`kvt.l2_load` names the time a token takes at `lmc`'s bandwidth",
    );
    refused(
        &with(cache, "kvt.l2_load (lmc, kv, 20, rate = 1);"),
        "`kvt.l2_load` names the time a token takes at `lmc`'s bandwidth: `per_token = …`, not `rate = …`",
    );
    // the store's copy is a hold of its own, where `cached` is the cache's:
    // `cached + 100` would read the entry, not the enclosing hold (#522 review)
    refused(
        &with(
            cache,
            "kvt.l2_store (kv, lmc, cached + 100, per_token = 0.01);",
        ),
        "`kvt.l2_store` reads `cached`, which inside the copy would be the cache's entry",
    );
    refused(
        &with(cache, "kvt.l2_move (lmc, kv, 20, per_token = 1);"),
        "std/kvt has no statement `l2_move`",
    );
    refused(
        &with(cache, load).replacen("use \"std/kvt\";", "", 1),
        "`kvt.l2_load` needs `use \"std/kvt\";`",
    );
}

/// An engine's request copies in its L2 at the cache's bandwidth: 50 tokens
/// at 0.01 s take 0.5 s.
#[test]
fn an_engines_l2_load_takes_its_copy_time() {
    let r = run(
        &format!(
            "{CACHE}
             device gpu {{ kv cap 1000; }}
             pool kv on gpu {{ }}
             engine e on gpu {{
               tokens cap inf;
               request (p) {{
                 set t0 = now;
                 set got = min(cachedin(lmc.kv), 50);
                 kvt.l2_load (lmc, kv, got, per_token = 0.01);
                 observe load_time = now - t0;
                 prefill (p - got) growing kv;
                 kvt.l2_store (kv, lmc, p, per_token = 0.01);
               }}
               schedule {{ advance running; admit waiting while (running.preempted == 0) taking kv (p); }}
               execute (1);
             }}
             workload {{ arrive batch(1); turn {{ }} session {{ turn; tool (50); turn; end; }} }}
             server {{ e (100); }}"
        ),
        &common::horizon(1000.0),
    );
    let t = samples(&r, "load_time");
    assert!(t[0].abs() < 1e-9 && (t[1] - 0.5).abs() < 1e-9, "{t:?}");
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
    let src = std::fs::read_to_string("examples/multi-turn/vllm_lmcache.sq").unwrap();
    let mut o = common::horizon(6000.0);
    o.set("Lambda", "0.005").unwrap();
    let r = run_source(&src, &o, None).unwrap_or_else(|e| panic!("{e}"));
    let ttft = &r.client("ttft").unwrap().samples;
    let worst = ttft.iter().cloned().fold(0.0, f64::max);
    assert!(worst < 1.0, "the slowest first token took {worst} s");
}
