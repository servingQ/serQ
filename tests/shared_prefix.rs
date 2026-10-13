//! A hold's `prefix`: a chain of keyed segments, shared across sessions in
//! the pool's prefix cache (#374, `docs/design/shared-prefix-cache.md`).

mod common;

use serq::{Overrides, run_source};

fn run(src: &str, options: &Overrides) -> serq::Report {
    run_source(&common::main_source(src), options, None).unwrap()
}

fn options(horizon: f64) -> Overrides {
    Overrides {
        warmup: Some(0.0),
        seed: Some(1),
        ..common::horizon(horizon)
    }
}

/// A stage on a pool of `cap`; `N` sessions, each a 1600-token request
/// whose first 1200 tokens are the system prompt `sys`. Session `k` is sent
/// at `STAGGER`; the request's hold reads its chain with `PREFIX`.
fn program(cap: u32, n: u32, stagger: &str, prefix: &str) -> String {
    format!(
        r#"
        device gpu {{ kv cap {cap}; }}
        stage llm : delay;
        pool kv on gpu {{ block 16; evict lru; }}
        stage user : delay;
        workload {{ arrive batch({n}); init {{ set sys = 7; }}
          session {{ turn; }}
        }}
        server {{
          run user (cost(user, {stagger}));
          hold kv (cost(kv, 1600)) {prefix} {{
            observe got = cached;
            observe together = holders(kv);
            run llm (cost(llm, 1));
          }} cache (cost(kv, 1600));
        }}
"#
    )
}

const CHAIN: &str = "prefix (sys: 1200, serial: inf)";

/// Sessions one after another: the first computes the system prompt, and
/// every later one reads its 1200 tokens from the cache, though no session
/// sent it before. Without `prefix` each session's cache is its own, and
/// nothing is read.
#[test]
fn a_system_prompt_is_computed_once() {
    let shared = run(&program(100_000, 5, "serial * 10", CHAIN), &options(100.0));
    assert_eq!(
        shared.observe("got").unwrap().samples,
        [0.0, 1200.0, 1200.0, 1200.0, 1200.0],
        "{}",
        shared.text()
    );
    let own = run(&program(100_000, 5, "serial * 10", ""), &options(100.0));
    assert_eq!(own.observe("got").unwrap().samples, [0.0; 5]);
}

/// Three sessions read the cached system prompt at once, on a pool that
/// holds the prompt once and three private tails (1200 + 3 · 400 = 2400),
/// not three whole requests (4800). The prompt is allocated once, however
/// many holds read it (vLLM's `ref_cnt`); the run's conservation check
/// (debug builds) holds the pool's `used` to that.
#[test]
fn holds_reading_one_prefix_allocate_it_once() {
    let r = run(
        &program(2400, 4, "serial > 0 ? 10 : 0", CHAIN),
        &options(100.0),
    );
    let got = &r.observe("got").unwrap().samples;
    assert_eq!(got, &[0.0, 1200.0, 1200.0, 1200.0], "{}", r.text());
    let together = &r.observe("together").unwrap().samples;
    assert_eq!(
        together.iter().cloned().fold(0.0, f64::max),
        3.0,
        "the three later sessions hold the pool at once: {together:?}\n{}",
        r.text()
    );
}

/// Eviction takes leaves: a session's own tail goes before the system
/// prompt it hangs off, which stays cached for every later session. The
/// pool (1800) holds the prompt, one tail and one request's own 400, so
/// every admission evicts the tail before it.
#[test]
fn eviction_takes_the_tail_before_the_prompt() {
    let r = run(&program(1800, 6, "serial * 10", CHAIN), &options(100.0));
    assert_eq!(
        r.observe("got").unwrap().samples,
        [0.0, 1200.0, 1200.0, 1200.0, 1200.0, 1200.0],
        "{}",
        r.text()
    );
}

/// A hold without the clause has the chain `(serial: ∞)`: a chain of the
/// session's serial alone, longer than any sequence here, runs the same,
/// sample for sample, on the multi-turn request of #230. Written out, the
/// default is a second spelling of no clause, and does not link.
#[test]
fn the_own_chain_is_the_default() {
    let src = |prefix: &str| {
        format!(
            r#"
        device gpu {{ kv cap 4000; }}
        stage llm : delay;
        pool kv on gpu {{ block 16; evict lru; }}
        stage user : delay;
        workload {{ arrive closed(3); session {{ turn; }} }}
        server {{ loop {{ run user (cost(user, ~exp(1)));
          hold kv (cost(kv, min(cachedin(kv), 992) + 8)) {prefix} {{
            observe got = cached;
            run llm (cost(llm, 1));
          }} cache (cost(kv, 1000)); }}
        }}
"#
        )
    };
    let own = run(&src(""), &options(50.0));
    let chain = run(&src("prefix (serial: 1e12)"), &options(50.0));
    let a = &own.observe("got").unwrap().samples;
    assert!(a.iter().any(|&x| x > 0.0), "the program must hit: {a:?}");
    assert_eq!(a, &chain.observe("got").unwrap().samples);
    let error = run_source(
        &common::main_source(&src("prefix (serial: inf)")),
        &options(50.0),
        None,
    )
    .unwrap_err();
    assert!(error.contains("leave the clause out"), "{error}");
}

/// A key names content: it is read while the hold waits and when it is
/// admitted, and must be the same at both. One that reads the state, the
/// clock or a draw does not link; nor a chain on a pool that spills.
#[test]
fn a_key_reads_no_state() {
    for (prefix, said) in [
        ("prefix (now: 1200)", "a `prefix` segment's key reads"),
        (
            "prefix (sys: used(kv))",
            "a `prefix` segment's length reads",
        ),
        ("prefix (~exp(1): 1200)", "a `prefix` segment's key reads"),
    ] {
        let error = run_source(
            &common::main_source(&program(100_000, 1, "0", prefix)),
            &options(10.0),
            None,
        )
        .unwrap_err();
        assert!(error.contains(said), "{prefix}: {error}");
    }
    let spills = program(100_000, 1, "0", CHAIN).replace(
        "pool kv on gpu { block 16; evict lru; }",
        "pool kv on gpu { block 16; evict lru; spill l2 via link (1) when (1); }
         pool l2 { cap 1000000; block 16; evict lru; } stage link : delay;",
    );
    let error = run_source(&common::main_source(&spills), &options(10.0), None).unwrap_err();
    assert!(error.contains("not supported yet"), "{error}");
}

/// Three sessions on the system prompt, one after another, then each a
/// second request on its own context, in reverse order. `INNER` stands in
/// the first request's body, `REUSE` bounds its read.
fn two_requests(reuse: &str, inner: &str) -> String {
    format!(
        r#"
        device gpu {{ kv cap 100000; }}
        stage llm : delay;
        pool kv on gpu {{ block 16; evict lru; }}
        stage user : delay;
        workload {{ arrive batch(3); init {{ set sys = 7; }} session {{ turn; }} }}
        server {{
          run user (cost(user, serial * 10));
          hold kv (cost(kv, 1600)) {CHAIN} {reuse} {{
            run llm (cost(llm, 1));
            {inner}
          }} cache (cost(kv, 1600));
          run user (cost(user, 40 - serial * 10));
          hold kv (cost(kv, 2000)) {CHAIN} {{
            observe second = cached;
            run llm (cost(llm, 1));
          }} cache (cost(kv, 2000));
        }}
"#
    )
}

/// One session's `reuse` bounds its own read, and no other session's cache.
/// Session 2 reads at most 800 of the system prompt; the cut it makes past
/// 800 used to take the system prompt's subtree off the trie, with the
/// other sessions' own contexts in it, so their second requests found 1200,
/// not 1600 (the review of #374). The cut takes only what the session wrote
/// alone.
#[test]
fn a_reuse_bound_cuts_no_other_session() {
    let r = run(
        &two_requests("reuse (cost(kv, serial == 2 ? 800 : 1e9))", ""),
        &options(200.0),
    );
    assert_eq!(
        r.observe("second").unwrap().samples,
        [1600.0, 1600.0, 1600.0],
        "{}",
        r.text()
    );
}

/// A hold nested in one that reads the shared system prompt, of the same
/// session, writing its chain. The system prompt is not the session's
/// consumed entry (#230), though the session's holds alone read it at that
/// moment: other sessions' contexts hang off it. Displacing it used to
/// leave them unreachable, and session 0's second request found 1200; the
/// nested hold read nothing (the review of #374). It reads the prompt, and
/// the others keep theirs.
#[test]
fn a_shared_prefix_is_no_session_s_consumed_entry() {
    let inner = "if (serial == 1) {
                   hold kv (cost(kv, 1216)) prefix (sys: 1200, serial: inf) {
                     observe inner = cached;
                   } cache (cost(kv, 1600));
                 }";
    let r = run(&two_requests("", inner), &options(200.0));
    assert_eq!(
        r.observe("inner").unwrap().samples,
        [1200.0],
        "{}",
        r.text()
    );
    assert_eq!(
        r.observe("second").unwrap().samples,
        [1600.0, 1600.0, 1600.0],
        "{}",
        r.text()
    );
}

/// What a hold reads is part of what it allocates. A request that
/// allocates 160 on a pool whose cached system prompt (1200) another hold
/// reads used to read all 1200 and add 160 - 1200 to the pool's `used`,
/// which went negative (the review of #374). It reads what its allocation
/// holds, in whole blocks.
#[test]
fn a_hold_reads_at_most_what_it_allocates() {
    let src = r#"
        device gpu { kv cap 100000; }
        pool kv on gpu { block 16; evict lru; }
        stage user : delay;
        workload { arrive batch(3); init { set sys = 7; } session { turn; } }
        server {
          run user (cost(user, serial == 0 ? 0 : 10));
          hold kv (cost(kv, serial == 0 ? 1600 : 160)) prefix (sys: 1200, serial: inf) {
            observe got = cached;
            run user (cost(user, 1));
            observe u = used(kv);
          } cache (cost(kv, serial == 0 ? 1600 : 16));
        }
"#;
    let r = run(src, &options(200.0));
    assert_eq!(
        r.observe("got").unwrap().samples,
        [0.0, 160.0, 160.0],
        "{}",
        r.text()
    );
    let used = &r.observe("u").unwrap().samples;
    assert!(
        used.iter().all(|&u| u >= 0.0),
        "used {used:?}\n{}",
        r.text()
    );
}

/// A key is the attribute it reads and its value: `prefix (sys: 1200, …)`
/// with `sys = 1` is not the entry of the session whose serial is 1, which
/// it used to read (the review of #374).
#[test]
fn a_key_is_its_attribute_and_its_value() {
    let src = format!(
        r#"
        device gpu {{ kv cap 100000; }}
        stage llm : delay;
        pool kv on gpu {{ block 16; evict lru; }}
        stage user : delay;
        workload {{ arrive batch(3); init {{ set sys = 1; }} session {{ turn; }} }}
        server {{
          run user (cost(user, serial * 10));
          if (serial == 1) {{
            hold kv (cost(kv, 1600)) {{
              run llm (cost(llm, 1));
            }} cache (cost(kv, 1600));
          }}
          if (serial == 2) {{
            hold kv (cost(kv, 1600)) {CHAIN} {{
              observe got = cached;
              run llm (cost(llm, 1));
            }} cache (cost(kv, 1600));
          }}
        }}
"#
    );
    let r = run(&src, &options(100.0));
    assert_eq!(r.observe("got").unwrap().samples, [0.0], "{}", r.text());
}

/// `N` sessions with `sys = 7` on a stage and a pool of `cap` (block 16),
/// the server's statements `server`.
fn deployment(cap: u32, n: u32, pools: &str, server: &str) -> String {
    format!(
        r#"
        device gpu {{ kv cap {cap}; }}
        stage llm : delay;
        pool kv on gpu {{ block 16; evict lru; preempt lifo; }}
        {pools}
        stage user : delay;
        workload {{ arrive batch({n}); init {{ set sys = 7; }} session {{ turn; }} }}
        server {{ {server} }}
"#
    )
}

/// The session that computed the system prompt first has no more say over
/// it than any other. Session 0 writes it; session 1 reads it; session 0
/// then sends a short request on the same chain (320 units, or 1600 kept as
/// 320): the shared prompt used to be cut, or trimmed, to 320, as the first
/// writer's own entry, and session 2 read 320 (the second review of #374).
/// A session's entry is what hangs under its own serial's key.
#[test]
fn the_first_writer_does_not_own_the_shared_prompt() {
    for (units, keep) in [(320, 320), (1600, 320)] {
        let src = deployment(
            100_000,
            3,
            "",
            &format!(
                "run user (cost(user, serial * 10));
                 hold kv (cost(kv, serial == 1 ? 1200 : 1600)) {CHAIN} {{
                   if (serial == 2) {{ observe late = cached; }}
                   run llm (cost(llm, 1));
                 }} cache (cost(kv, 1600));
                 if (serial == 0) {{
                   run user (cost(user, 15));
                   hold kv (cost(kv, {units})) {CHAIN} {{
                     observe short = cached;
                   }} cache (cost(kv, {keep}));
                 }}"
            ),
        );
        let r = run(&src, &options(100.0));
        assert_eq!(
            r.observe("late").unwrap().samples,
            [1200.0],
            "units {units}, cache {keep}: {}",
            r.text()
        );
    }
}

/// `drop kv` takes the session's own entry, not a system prompt it
/// happened to compute first: session 1 used to read 0 after session 0's
/// drop (the second review of #374).
#[test]
fn drop_takes_the_session_s_entry_alone() {
    let src = deployment(
        100_000,
        3,
        "",
        &format!(
            "run user (cost(user, serial * 10));
             hold kv (cost(kv, 1600)) {CHAIN} {{
               if (serial == 1) {{ observe mid = cached; }}
               run llm (cost(llm, 1));
             }} cache (cost(kv, 1600));
             run user (cost(user, 1));
             if (serial == 0) {{ drop kv; }}"
        ),
    );
    let r = run(&src, &options(100.0));
    assert_eq!(r.observe("mid").unwrap().samples, [1200.0], "{}", r.text());
}

/// Two legs of one session read one system prompt at once, as two vLLM
/// requests do (`ref_cnt`): the second leg used to find none while the
/// first held it, unless another session's context hung off it (the second
/// review of #374). The session's own context, which the first leg reads,
/// is its consumed entry (#230); the prompt is not.
#[test]
fn two_legs_read_one_prompt() {
    let src = deployment(
        100_000,
        1,
        "",
        &format!(
            "hold kv (cost(kv, 1600)) {CHAIN} {{
               run llm (cost(llm, 1));
             }} cache (cost(kv, 1600));
             fork {{
               hold kv (cost(kv, 2000)) {CHAIN} {{
                 observe a = cached;
                 run user (cost(user, 5));
               }} cache (cost(kv, 2000));
             }}
             fork {{
               run user (cost(user, 1));
               hold kv (cost(kv, 2000)) {CHAIN} {{
                 observe b = cached;
                 run llm (cost(llm, 1));
               }} cache (cost(kv, 2000));
             }}
             join;"
        ),
    );
    let r = run(&src, &options(100.0));
    assert_eq!(r.observe("a").unwrap().samples, [1600.0], "{}", r.text());
    assert_eq!(r.observe("b").unwrap().samples, [1200.0], "{}", r.text());
}

/// A segment of length 0 holds nothing: `prefix (sys: 0, serial: inf)` is
/// the session's own chain, and a request reads its context back. The walk
/// used to stop at the empty segment, so nothing was ever read and the
/// session's entries piled up (the second review of #374).
#[test]
fn an_empty_segment_is_left_out() {
    let src = deployment(
        100_000,
        1,
        "",
        "set ctx = 0;
         loop {
           set ctx = ctx + 400;
           hold kv (cost(kv, ctx)) prefix (sys: 0, serial: inf) {
             observe got = cached;
             run llm (cost(llm, 1));
           } cache (cost(kv, ctx));
           run user (cost(user, 1));
         }",
    );
    let r = run(&src, &options(9.5));
    assert_eq!(
        r.observe("got").unwrap().samples,
        [0.0, 400.0, 800.0, 1200.0, 1600.0],
        "{}",
        r.text()
    );
}

/// A slot pool held with a chain keeps its slots: its `cache` holds a slot,
/// per session, not a shared segment every hold would read as allocated
/// already. Five sessions on two slots used to hold them all at once (the
/// second review of #374).
#[test]
fn a_chain_leaves_a_slot_pool_its_cap() {
    let src = deployment(
        100_000,
        6,
        "pool slots { cap 2; }",
        &format!(
            "run user (cost(user, serial == 0 ? 0 : 10));
             hold slots (cost(slots, 1)), kv (cost(kv, 1600)) {CHAIN} {{
               observe together = holders(slots);
               run user (cost(user, 5));
             }} cache (cost(slots, kv, 1600));"
        ),
    );
    let r = run(&src, &options(100.0));
    let together = &r.observe("together").unwrap().samples;
    assert!(
        together.iter().all(|&t| t <= 2.0),
        "{together:?}\n{}",
        r.text()
    );
    let error = run_source(
        &common::main_source(&src.replace(
            &format!("slots (cost(slots, 1)), kv (cost(kv, 1600)) {CHAIN}"),
            &format!("slots (cost(slots, 1)) {CHAIN}"),
        )),
        &options(10.0),
        None,
    )
    .unwrap_err();
    assert!(error.contains("no pool with `block`"), "{error}");
}

/// Without `prefix` a hold consumes its session's entry whole, though it
/// allocates less, and what its allocation does not hold is gone, as the
/// Lean model's `admitPool` says; the bound of a read by the allocation is
/// the shared chain's (the second review of #374 found it applied to every
/// hold).
#[test]
fn the_own_chain_is_consumed_whole() {
    let src = deployment(
        3000,
        1,
        "",
        "hold kv (cost(kv, 1600)) {
           run llm (cost(llm, 1));
         } cache (cost(kv, 1600));
         run user (cost(user, 1));
         hold kv (cost(kv, 320)) {
           observe got = cached;
         } cache (cost(kv, 320));",
    );
    let r = run(&src, &options(100.0));
    assert_eq!(r.observe("got").unwrap().samples, [1600.0], "{}", r.text());
}

/// Holds reading a shared prompt are preempted and admitted again: four
/// sessions grow past what the pool holds (`grow kv`), the newest is preempted
/// (`preempt lifo`), and each reads the prompt again when it is admitted.
/// The run's conservation check (debug builds) holds through it.
#[test]
fn a_preempted_reader_reads_the_prompt_again() {
    let src = deployment(
        4000,
        5,
        "",
        &format!(
            "run user (cost(user, serial == 0 ? 0 : 10));
             hold kv (cost(kv, 1600)) {CHAIN} {{
               observe got = cached;
               run llm (cost(llm, 1));
               grow kv (cost(kv, 600)); run llm (cost(llm, 1));
             }} cache (cost(kv, 2200));"
        ),
    );
    let r = run(&src, &options(200.0));
    let kv = r.pools.iter().find(|p| p.name == "kv").unwrap();
    assert!(kv.preemptions > 0, "{}", r.text());
    let got = &r.observe("got").unwrap().samples;
    assert_eq!(got[0], 0.0, "{}", r.text());
    assert!(
        got[1..].iter().all(|&g| g >= 1200.0),
        "every later admission reads the prompt: {got:?}\n{}",
        r.text()
    );
}

/// A session's entry is under its own serial, the number the run gives
/// it. A program that set every session's `serial` to 0 keyed their chains
/// by 0, and whether session 0's `drop` took session 1's context depended
/// on which wrote first (the third and fourth reviews of #374): `serial`
/// is not set, in the workload either.
#[test]
fn serial_is_not_set() {
    let src = r#"
        device gpu { kv cap 100000; }
        pool kv on gpu { block 16; evict lru; }
        stage user : delay;
        workload { arrive batch(3); init { set d = serial; set serial = 0; } session { turn; } }
        server {
          run user (cost(user, d * 10));
          hold kv (cost(kv, 1600)) prefix (serial: 1e12) {
            observe got = cached;
          } cache (cost(kv, 1600));
        }
"#;
    let error = run_source(&common::main_source(src), &options(100.0), None).unwrap_err();
    assert!(
        error.contains("`serial` is the session's number"),
        "{error}"
    );
}

/// A request's chain is its first item, read before the request does
/// anything: a name the request sets is not yet set there, and the chain
/// may not read it (the fourth review of #374).
#[test]
fn a_request_s_chain_reads_nothing_the_request_sets() {
    let src = r#"
        device gpu { kv cap 100000; }
        pool kv on gpu { block 16; evict lru; preempt lifo; }
        engine llm on gpu {
          tokens cap 8192;
          request (n, product, system, session) {
            prefix (product: system, session: k);
            set k = n;
            at admission { reused = min(cachedin(kv), floor(n - 1, 16)); }
            prefill (n - cached) growing kv;
            cache (n);
          }
          scheduler { advance running; admit waiting while (running.preempted == 0) taking kv (n); }
          execute (batch.tokens * 1e-5 + 1e-4);
        }
        workload { arrive batch(1); init { set sys = 7; } session { turn; } }
        server { llm (1600, sys, 1200, serial); }
"#;
    let error = run_source(&common::main_source(src), &options(10.0), None).unwrap_err();
    assert!(error.contains("which the request sets after it"), "{error}");
}

/// A chain says what a hold reads and keeps of the cache, which a hold
/// without `cache` does neither of: the linker refuses it, as it refuses
/// `reuse` there (the third review of #374). Only the last segment may be
/// endless: past an endless one no other is reached, and the sessions'
/// contexts piled into the first.
#[test]
fn a_chain_without_cache_or_with_an_endless_middle_does_not_link() {
    let base = program(100_000, 1, "0", CHAIN);
    for (src, said) in [
        (
            base.replace("}} cache (cost(kv, 1600));", "}}")
                .replace("} cache (cost(kv, 1600));", "}"),
            "no `cache` clause",
        ),
        (
            program(100_000, 1, "0", "prefix (sys: inf, serial: inf)"),
            "other than the last is endless",
        ),
    ] {
        let error = run_source(&common::main_source(&src), &options(10.0), None).unwrap_err();
        assert!(error.contains(said), "{error}\n{src}");
    }
}

/// An engine's request says what its prompt is made of, its first item:
/// `prefix (product: system, session: inf);`, read by the hold of its
/// admission. The system prompt is computed once, by the first session,
/// and every later one reads it, prefilling only its own (#374).
#[test]
fn an_engine_s_request_takes_a_chain() {
    let src = |prefix: &str| {
        format!(
            r#"
        device gpu {{ kv cap 100000; }}
        pool kv on gpu {{ block 16; evict lru; preempt lifo; }}
        engine llm on gpu {{
          tokens cap 8192;
          request (n, product, system, session) {{
            {prefix}
            at admission {{ reused = min(cachedin(kv), floor(n - 1, 16)); }}
            set c = min(cached, floor(n - 1, 16));
            observe got = c;
            prefill (n - c) growing kv;
            cache (n);
          }}
          scheduler {{
            advance running;
            admit waiting while (running.preempted == 0) taking kv (n) ;
          }}
          execute (batch.tokens * 1e-5 + 1e-4);
        }}
        stage user : delay;
        workload {{ arrive batch(4); init {{ set sys = 7; }} session {{ turn; }} }}
        server {{
          run user (cost(user, serial * 10));
          llm (1600, sys, 1200, serial);
        }}
"#
        )
    };
    let r = run(
        &src("prefix (product: system, session: inf);"),
        &options(100.0),
    );
    assert_eq!(
        r.observe("got").unwrap().samples,
        [0.0, 1200.0, 1200.0, 1200.0],
        "{}",
        r.text()
    );
    let own = run(&src(""), &options(100.0));
    assert_eq!(own.observe("got").unwrap().samples, [0.0; 4]);
    for (body, said) in [
        (
            "set x = 1; prefix (product: system, session: inf);",
            "is its first item",
        ),
        ("prefix (product: system, session: inf)", "ends with `;`"),
    ] {
        let error = run_source(&common::main_source(&src(body)), &options(10.0), None).unwrap_err();
        assert!(error.contains(said), "{body}: {error}");
    }
}

/// SGLang's LPM order over the shared cache: `queue by (-cachedin(kv))`
/// admits the waiting request whose chain the cache holds most of.
/// Session 0 computes system prompt 7 and holds the pool; sessions 1
/// (system prompt 9) and 2 (system prompt 7) wait, in that order. Room
/// for one: FIFO admits session 1, which evicts the prompt; LPM admits
/// session 2, which reads it (examples/engines/sglang.sq `--lpm 1`).
#[test]
fn lpm_admits_the_longest_cached_chain_first() {
    let src = |queue: &str| {
        format!(
            r#"
        device gpu {{ kv cap 2000; }}
        pool kv on gpu {{ block 16; evict lru; {queue} }}
        stage user : delay;
        workload {{ arrive batch(3); init {{ set sys = serial == 1 ? 9 : 7; }} session {{ turn; }} }}
        server {{
          run user (cost(user, serial == 0 ? 0 : 1));
          hold kv (cost(kv, 1600)) {CHAIN} {{
            observe start = now;
            observe got = cached;
            run user (cost(user, 10));
          }} cache (cost(kv, 1600));
        }}
"#
        )
    };
    // session `s`'s observation `name`
    let of = |r: &serq::Report, name: &str, s: u64| {
        let o = r.observe(name).unwrap();
        o.records
            .iter()
            .zip(&o.samples)
            .find(|(rec, _)| rec.1 == s)
            .map(|(_, v)| *v)
            .unwrap()
    };
    let fifo = run(&src(""), &options(100.0));
    assert!(
        of(&fifo, "start", 1) < of(&fifo, "start", 2),
        "{}",
        fifo.text()
    );
    let lpm = run(&src("queue by (-cachedin(kv));"), &options(100.0));
    assert!(
        of(&lpm, "start", 2) < of(&lpm, "start", 1),
        "{}",
        lpm.text()
    );
    assert_eq!(of(&lpm, "got", 2), 1200.0, "{}", lpm.text());
}

/// A queue's entry keys the session's own context by the session's
/// number, `serial`: a `prefix` key reads it, and no other session
/// attribute, and no other part of the header
/// (examples/multi-turn/thunderagent.sq).
#[test]
fn an_entry_keys_its_prefix_by_the_requests_serial() {
    let entry = |key: &str, units: &str| {
        format!(
            r#"
        queue E : decode {{
          device gpu {{ kv cap 640; }}
          engine on gpu {{ reqs cap 4; tokens cap 64; scheduler {{ advance running; admit waiting while (running.preempted == 0); }} execute (1); }}
          pool reqs on E {{ }}
          pool kv on E.gpu {{ block 16; }}
          decode (prompt) {{
            hold kv (cost(kv, {units})), reqs (cost(reqs, 1)) prefix (0: 32, {key}: inf) {{
              run E prefill (cost(E, prompt)) growing kv;
            }} cache (cost(kv, reqs, prompt));
          }}
        }}
        workload {{ arrive batch(2); init {{ set other = 1; }} session {{ turn; }} }}
        server {{ E.decode (48); }}
"#
        )
    };
    serq::check_source(
        &common::main_source(&entry("serial", "prompt")),
        &common::horizon(10.0),
    )
    .unwrap();
    let refused = |key: &str, units: &str| {
        serq::check_source(
            &common::main_source(&entry(key, units)),
            &common::horizon(10.0),
        )
        .unwrap_err()
    };
    let e = refused("other", "prompt");
    assert!(
        e.contains("the header reads `other`") && e.contains("mark `other` `hidden`"),
        "{e}"
    );
    let e = refused("serial", "prompt + serial");
    assert!(
        e.contains("the header reads `serial`")
            && e.contains("A `prefix` key reads `serial` as it is"),
        "{e}"
    );
}
