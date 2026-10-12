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
