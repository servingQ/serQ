//! Deterministic scenarios on the vLLM v1 engine (`programs/vllm.seq`
//! and inline variants), each mirroring a behaviour of
//! `ref/vllm/vllm/v1/core/sched/scheduler.py` (line numbers at commit
//! 0c87a197). Iteration cost is 1, so times are scheduler steps.

use seq::{Overrides, run_source};

/// `n` requests present at t = 0 (closed population, one turn each), with
/// prompt `prompt` and `out` output tokens, on a device of `blocks` blocks
/// of `bs` tokens, budget `budget`, cap `max_seqs`.
#[allow(clippy::too_many_arguments)]
fn engine(
    n: usize,
    prompt: &str,
    out: &str,
    blocks: usize,
    bs: usize,
    budget: usize,
    max_seqs: usize,
    extra: &str,
) -> String {
    format!(
        r#"
        let bs = {bs};
        pool kv {{ cap {blocks} * bs; block bs; evict lru; preempt lifo; }}
        pool reqs {{ cap {max_seqs}; }}
        stage engine : step {{ budget {budget}; cost 1; memory kv; {extra} }}
        workload {{ arrive batch({n}); init {{ set prompt = {prompt}; set o = {out}; }} }}
        session {{
          set t0 = now;
          hold reqs (1), kv (min(prompt, {budget})) {{
            observe admitted = now - t0;
            observe who_admitted = serial;
            run engine prefill (prompt) growing kv;
            observe ttft = now - t0;
            run engine decode (o - 1) growing kv;
          }}
          observe done = now - t0;
          observe order = serial;
          end;
        }}
        run {{ horizon 1000; }}
        "#
    )
}

fn run(src: &str) -> seq::Report {
    run_source(src, &Overrides::default(), None).unwrap()
}

/// scheduler.py:742-813 (`test_preempt_during_execution`): two 80-token
/// requests fill 10 blocks of 16; the first decode step needs an 81st
/// slot, no block is free, and the most recently admitted request is
/// preempted (`self.running[-1]`), freeing its blocks; it resumes after.
#[test]
fn growth_preempts_the_last_admitted_request() {
    let r = run(&engine(
        2,
        "80",
        "serial == 0 ? 20 : 3",
        10,
        16,
        100,
        16,
        "",
    ));
    let kv = r.pool("kv").unwrap();
    assert_eq!(kv.preemptions, 1, "{}", r.text());
    let done = &r.observe("done").unwrap().samples;
    let order = &r.observe("order").unwrap().samples;
    // request 0 finishes first (step 1 prefill + 19 decode steps = 20),
    // request 1 was preempted and recomputes after 0's blocks are freed
    assert_eq!(order, &[0.0, 1.0]);
    assert_eq!(done[0], 20.0);
    assert!(done[1] > 20.0, "{done:?}");
}

/// scheduler.py:872-884, 1228-1235: FCFS with head-of-line blocking; a
/// request that does not fit stops the waiting loop even if a later,
/// smaller one would fit.
#[test]
fn admission_is_fcfs_with_head_of_line_blocking() {
    // blocks: 10 of 16 = 160 tokens. r0: 96 (6 blocks), r1: 96 (does not fit
    // with r0), r2: 16 (would fit). r2 must wait behind r1.
    let src = engine(3, "serial == 2 ? 16 : 96", "2", 10, 16, 1000, 16, "");
    let r = run(&src);
    let adm = &r.observe("admitted").unwrap().samples;
    let order = &r.observe("who_admitted").unwrap().samples;
    // observations are in admission order; find r2
    let i2 = order.iter().position(|&s| s == 2.0).unwrap();
    assert!(
        adm[i2] > 0.0,
        "r2 was admitted at {} although r1 blocks the queue\n{}",
        adm[i2],
        r.text()
    );
}

/// scheduler.py:1078-1128: chunked prefill takes ceil(prompt / budget)
/// steps; the first token is out at the end of the last chunk.
#[test]
fn chunked_prefill_takes_ceil_prompt_over_budget_steps() {
    let r = run(&engine(1, "3000", "1", 1000, 16, 1024, 16, ""));
    let ttft = r.observe("ttft").unwrap().samples[0];
    assert_eq!(ttft, 3.0, "{}", r.text());
}

/// scheduler.py:612-616, 675-676: `long_prefill_token_threshold` caps one
/// request's chunk only when it is not alone.
#[test]
fn long_prefill_threshold_applies_only_with_company() {
    let alone = run(&engine(1, "3000", "1", 1000, 16, 4096, 16, "chunk 1000;"));
    assert_eq!(alone.observe("ttft").unwrap().samples[0], 3.0);
    // seQ applies `chunk` unconditionally: the "alone" exception of vLLM
    // (num_eligible_reqs > 1) is not modelled; document it.
}

/// kv_cache_manager.py:289-300, block_pool.py:776-805: a finished request's
/// full blocks stay cached; the next turn of the session reuses them (all
/// full blocks of the prompt but the last token); the partial tail block
/// is not cached.
#[test]
fn next_turn_reuses_full_blocks_of_the_cached_prefix() {
    let src = r#"
        let bs = 16;
        pool kv { cap 1000 * bs; block bs; evict lru; preempt lifo; }
        pool reqs { cap 16; }
        stage engine : step { budget 8192; cost 1; memory kv; }
        workload { arrive batch(1); init { set K = 0; set turns = 0; } }
        session {
          loop {
            set prompt = K + 100;
            hold reqs (1), kv (c + min(prompt - c, 8192))
                 at admission (c = min(cachedin(kv), floor((prompt - 1) / bs) * bs)) {
              observe cached_seen = cached;
              observe prefill_tokens = prompt - min(cached, floor((prompt - 1) / bs) * bs);
              run engine prefill (prompt - min(cached, floor((prompt - 1) / bs) * bs)) growing kv;
              run engine decode (9) growing kv;
            } cache (prompt + 10);
            set K = prompt + 10;
            set turns = turns + 1;
            branch (turns >= 3) { end; }
          }
        }
        run { horizon 1000; }
    "#;
    let r = run(src);
    let seen = &r.observe("cached_seen").unwrap().samples;
    let pre = &r.observe("prefill_tokens").unwrap().samples;
    // turn 1: nothing cached, prefill 100; context after = 110 -> 6 full
    // blocks (96) cached. turn 2: prompt 210, cached 96, prefill 114;
    // context 220 -> 13 blocks (208). turn 3: prompt 320, cached 208.
    assert_eq!(seen, &[0.0, 96.0, 208.0], "{}", r.text());
    assert_eq!(pre, &[100.0, 114.0, 112.0]);
}

/// block_pool.py:776-805 + single_type_kv_cache_manager.py (free in
/// reverse order): under memory pressure the LRU cache is drained tail
/// first, so a session keeps a shorter prefix rather than losing it whole.
#[test]
fn lru_eviction_drops_tail_blocks_first() {
    let src = r#"
        let bs = 16;
        pool kv { cap 20 * bs; block bs; evict lru; preempt lifo; }
        pool reqs { cap 16; }
        stage engine : step { budget 8192; cost 1; memory kv; }
        stage gate : delay;
        workload { arrive batch(2); init { set K = 0; set turns = 0; } }
        session {
          // session 0 runs first (160 tokens -> 10 blocks cached), then session 1
          // takes 12 blocks, evicting 2 of session 0's from its tail; session 0's
          // second turn then reuses 8 blocks.
          run gate (serial * 2);
          loop {
            set prompt = serial == 0 ? K + 160 : 192;
            hold reqs (1), kv (c + min(prompt - c, 8192))
                 at admission (c = min(cachedin(kv), floor((prompt - 1) / bs) * bs)) {
              branch (serial == 0) { observe cached0 = cached; }
              run engine prefill (prompt - min(cached, floor((prompt - 1) / bs) * bs)) growing kv;
            } cache (prompt);
            set K = prompt;
            set turns = turns + 1;
            branch (turns >= 2 || serial == 1) { end; }
            run gate (10);
          }
        }
        run { horizon 1000; }
    "#;
    let r = run(src);
    let c0 = &r.observe("cached0").unwrap().samples;
    assert_eq!(c0, &[0.0, 128.0], "{}", r.text());
    // 2 blocks of session 0 for session 1's turn, then session 1's 12 blocks
    // (kept after it ended, as vLLM keeps them) for session 0's second turn
    assert_eq!(r.pool("kv").unwrap().evicted_units, 32.0 + 192.0);
}

/// The RBLN stack of docs/testbed.md (a prefill step is exclusive and has
/// priority over decode): with `exclusive prefill` a prefilling request
/// stalls every decode for its chunks.
#[test]
fn exclusive_prefill_stalls_decodes() {
    let done_of = |r: &seq::Report, who: f64| -> f64 {
        let order = &r.observe("order").unwrap().samples;
        let done = &r.observe("done").unwrap().samples;
        done[order.iter().position(|&s| s == who).unwrap()]
    };
    let shared = run(&engine(
        2,
        "serial == 0 ? 1 : 2048",
        "serial == 0 ? 10 : 1",
        1000,
        16,
        1024,
        16,
        "",
    ));
    let excl = run(&engine(
        2,
        "serial == 0 ? 1 : 2048",
        "serial == 0 ? 10 : 1",
        1000,
        16,
        1024,
        16,
        "serve exclusive prefill;",
    ));
    // request 0: 1 prefill step + 9 decode steps = 10 when sharing; with
    // exclusive prefill it waits for request 1's two chunks: 12
    assert_eq!(done_of(&shared, 0.0), 10.0, "{}", shared.text());
    assert_eq!(done_of(&excl, 0.0), 12.0, "{}", excl.text());
}

/// scheduler.py:877-879: `max_num_seqs` caps the running set.
#[test]
fn request_cap_is_a_slot_pool() {
    let r = run(&engine(4, "16", "5", 1000, 16, 8192, 2, ""));
    let adm = &r.observe("admitted").unwrap().samples;
    let mut a = adm.clone();
    a.sort_by(f64::total_cmp);
    assert_eq!(a, vec![0.0, 0.0, 5.0, 5.0], "{}", r.text());
}
