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

/// One engine on a pool of `cap`; `N` sessions, each a 1600-token request
/// whose first 1200 tokens are the system prompt `sys`. Session `k` is sent
/// at `STAGGER`; the request's hold reads its chain with `PREFIX`.
fn program(cap: u32, n: u32, stagger: &str, prefix: &str) -> String {
    format!(
        r#"
        device gpu {{ kv cap {cap}; }}
        engine llm on gpu {{
          tokens cap 8192;
          schedule {{ advance running; admit waiting while (running.preempted == 0); }}
          execute (batch.tokens * 1e-5 + 1e-4);
        }}
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
            run llm prefill (cost(llm, 1600 - cached)) growing kv;
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
        engine llm on gpu {{
          tokens cap 8192;
          schedule {{ advance running; admit waiting while (running.preempted == 0); }}
          execute (batch.tokens * 1e-5 + 1e-4);
        }}
        pool kv on gpu {{ block 16; evict lru; }}
        stage user : delay;
        workload {{ arrive closed(3); session {{ turn; }} }}
        server {{ loop {{ run user (cost(user, ~exp(1)));
          hold kv (cost(kv, min(cachedin(kv), 992) + 8)) {prefix} {{
            observe got = cached;
            run llm prefill (cost(llm, 1000 - cached)) growing kv;
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
        engine llm on gpu {{
          tokens cap 8192;
          schedule {{ advance running; admit waiting while (running.preempted == 0); }}
          execute (batch.tokens * 1e-5 + 1e-4);
        }}
        pool kv on gpu {{ block 16; evict lru; }}
        stage user : delay;
        workload {{ arrive batch(3); init {{ set sys = 7; }} session {{ turn; }} }}
        server {{
          run user (cost(user, serial * 10));
          hold kv (cost(kv, 1600)) {CHAIN} {reuse} {{
            run llm prefill (cost(llm, 1600 - cached)) growing kv;
            {inner}
          }} cache (cost(kv, 1600));
          run user (cost(user, 40 - serial * 10));
          hold kv (cost(kv, 2000)) {CHAIN} {{
            observe second = cached;
            run llm prefill (cost(llm, 2000 - cached)) growing kv;
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
    let inner = "branch (serial == 1) {
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
        engine llm on gpu {
          tokens cap 8192;
          schedule { advance running; admit waiting while (running.preempted == 0); }
          execute (batch.tokens * 1e-5 + 1e-4);
        }
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
        engine llm on gpu {{
          tokens cap 8192;
          schedule {{ advance running; admit waiting while (running.preempted == 0); }}
          execute (batch.tokens * 1e-5 + 1e-4);
        }}
        pool kv on gpu {{ block 16; evict lru; }}
        stage user : delay;
        workload {{ arrive batch(3); init {{ set sys = 1; }} session {{ turn; }} }}
        server {{
          run user (cost(user, serial * 10));
          branch (serial == 1) {{
            hold kv (cost(kv, 1600)) {{
              run llm prefill (cost(llm, 1600)) growing kv;
            }} cache (cost(kv, 1600));
          }}
          branch (serial == 2) {{
            hold kv (cost(kv, 1600)) {CHAIN} {{
              observe got = cached;
              run llm prefill (cost(llm, 1600 - cached)) growing kv;
            }} cache (cost(kv, 1600));
          }}
        }}
"#
    );
    let r = run(&src, &options(100.0));
    assert_eq!(r.observe("got").unwrap().samples, [0.0], "{}", r.text());
}
