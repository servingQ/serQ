//! Deterministic scenarios on the vLLM v1 engine (`examples/multi-turn/vllm.sq`
//! and inline variants), each mirroring a behaviour of
//! `ref/vllm/vllm/v1/core/sched/scheduler.py` (line numbers at commit
//! 0c87a197). Iteration cost is 1, so times are scheduler steps.

mod common;

use serq::{Overrides, run_source};

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
        workload {{ arrive batch({n}); init {{ set prompt = {prompt}; set o = {out}; }}
          session {{ request;
            end;

          }}
        }}
        server {{
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
        }}

"#
    )
}

fn run(src: &str, options: &Overrides) -> serq::Report {
    run_source(&common::main_source(src), options, None).unwrap()
}

/// scheduler.py:742-813 (`test_preempt_during_execution`): two 80-token
/// requests fill 10 blocks of 16; the first decode step needs an 81st
/// slot, no block is free, and the most recently admitted request is
/// preempted (`self.running[-1]`), freeing its blocks; it resumes after.
#[test]
fn growth_preempts_the_last_admitted_request() {
    let r = run(
        &engine(2, "80", "serial == 0 ? 20 : 3", 10, 16, 100, 16, ""),
        &common::horizon(1000.0),
    );
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

/// scheduler.py:1560-1561: `_preempt_request` resets `num_computed_tokens`
/// and keeps the request's output tokens, so a request preempted during
/// decode is rescheduled with `num_tokens = prompt + outputs`
/// (kv_cache_manager.py:515-531 reserves for that), recomputes their KV as
/// one prefill and generates only what is left. The re-executed hold reads
/// `computed`, the position it had computed, to say the same.
///
/// A (64 tokens, 20 out) and B (48, 40) are admitted at step 1 on 10 blocks
/// of 16. Both grow a block at step 2 (A 5, B 4; 1 free). At step 18 A
/// reaches token 81 and takes the last block; B needs its 65th slot and is
/// the last admitted: preempted with 64 tokens computed and 17 outputs
/// (one from the prefill, sixteen decodes). It needs 65 tokens = 5 blocks;
/// 4 are free until A finishes at step 20. Step 21: B recomputes 65 tokens
/// in one prefill (its 18th output), then decodes 22 more: done at 43.
/// Recomputing the prompt alone and every output again would end at 60.
#[test]
fn a_request_preempted_during_decode_resumes_from_its_outputs() {
    let src = r#"
        let bs = 16;
        pool kv { cap 10 * bs; block bs; evict lru; preempt lifo; }
        pool reqs { cap 16; }
        stage engine : step { budget 1000; cost 1; memory kv; }
        workload { arrive batch(2); init { set prompt = serial == 0 ? 64 : 48; set o = serial == 0 ? 20 : 40; }
          session { request;
            end;

          }
        }
        server {
          hold reqs (1), kv (min(known, 1000)) reserve (known)
          at admission (known = computed < prompt ? prompt : computed + 1) {
            observe known = known;
            run engine prefill (known) growing kv;
            branch (known == prompt) { observe first = now; }
            run engine decode (o - 1 - (known - prompt)) growing kv;
          }
          observe done = now;
          observe order = serial;
        }

"#;
    let r = run(src, &common::horizon(1000.0));
    let kv = r.pool("kv").unwrap();
    assert_eq!(kv.preemptions, 1, "{}", r.text());
    assert_eq!(kv.stuck, 0, "{}", r.text());
    assert_eq!(r.observe("known").unwrap().samples, vec![64.0, 48.0, 65.0]);
    // the first token is recorded once per request: B's re-prefill at 21 is
    // not a first token (vLLM's oracle records `first` once, too)
    assert_eq!(r.observe("first").unwrap().samples, vec![1.0, 1.0]);
    assert_eq!(r.observe("order").unwrap().samples, vec![0.0, 1.0]);
    assert_eq!(
        r.observe("done").unwrap().samples,
        vec![20.0, 43.0],
        "{}",
        r.text()
    );
}

/// A holder preempted before it computed anything resumes from nothing.
/// B holds 32 tokens of the same pool for a fifo stage and is the latest
/// admitted when A's decode needs a fifth block at step 2: B is the victim
/// with `computed` 0, not its 32-token allocation, so a program reading
/// `computed` does not invent an output token it never produced (vLLM's
/// `num_computed_tokens` is 0 for a request preempted before its first
/// step). The pool is no engine's memory here: with `memory kv` a holder
/// away from the engine is not in its `running` list and is not a victim.
#[test]
fn a_holder_preempted_before_its_first_step_has_computed_nothing() {
    let src = r#"
        let bs = 16;
        pool kv { cap 6 * bs; block bs; evict lru; preempt lifo; }
        stage engine : step { budget 1000; cost 1; }
        stage svc : fifo;
        workload { arrive batch(2); init { set prompt = serial == 0 ? 64 : 32; set o = 20; }
          session { request;
            end;

          }
        }
        server {
          branch (serial == 0) {
            hold kv (prompt) reserve (prompt) {
              run engine prefill (prompt) growing kv;
              run engine decode (o - 1) growing kv;
            }
          } else {
            hold kv (prompt) { observe c2 = computed; run svc (100); }
          }
          observe done = now;
        }

"#;
    let r = run(src, &common::horizon(200.0));
    assert_eq!(r.pool("kv").unwrap().preemptions, 1, "{}", r.text());
    // first execution, then the re-execution after A frees its blocks at 20
    assert_eq!(
        r.observe("c2").unwrap().samples,
        vec![0.0, 0.0],
        "{}",
        r.text()
    );
    assert_eq!(
        r.observe("done").unwrap().samples,
        vec![20.0, 120.0],
        "{}",
        r.text()
    );
}

/// `serve by (keys)`: the order the iteration hands its budget out in is an
/// expression over the residents, so a program can state a policy vLLM
/// does not have. With one token of budget per step, admission order gives
/// everything to A until it is done (A: prompt + 9 decodes = step 10, then
/// B: 11, 12, 13); shortest-remaining-first serves B as soon as it has
/// less left (B: prefill at 2, decodes at 3 and 4; A resumes and ends at
/// 13). `decode first` is `by (decoding ? 0 : 1)`.
#[test]
fn serve_by_orders_residents_by_the_declared_keys() {
    let prog = |serve: &str| {
        format!(
            "pool kv {{ cap 1000; }}
        stage engine : step {{ budget 1; cost 1; memory kv; {serve} }}
        workload {{ arrive batch(2); init {{ set o = serial == 0 ? 10 : 3; }}
          session {{ request;
            end;

          }}
        }}
        server {{
          hold kv (100) {{
            run engine prefill (1) growing kv;
            run engine decode (o - 1) growing kv;
          }}
          observe done = now;
          observe order = serial;
        }}
        "
        )
    };
    let r = run(&prog("serve admission;"), &common::horizon(100.0));
    assert_eq!(r.observe("order").unwrap().samples, vec![0.0, 1.0]);
    assert_eq!(
        r.observe("done").unwrap().samples,
        vec![10.0, 13.0],
        "{}",
        r.text()
    );
    let r = run(&prog("serve by (remaining);"), &common::horizon(100.0));
    assert_eq!(r.observe("order").unwrap().samples, vec![1.0, 0.0]);
    assert_eq!(
        r.observe("done").unwrap().samples,
        vec![4.0, 13.0],
        "{}",
        r.text()
    );
    // ties fall to admission order: a constant key is admission order, and
    // a second key decides where the first is equal
    let r = run(&prog("serve by (1);"), &common::horizon(100.0));
    assert_eq!(r.observe("order").unwrap().samples, vec![0.0, 1.0]);
    assert_eq!(r.observe("done").unwrap().samples, vec![10.0, 13.0]);
    let r = run(&prog("serve by (1, remaining);"), &common::horizon(100.0));
    assert_eq!(r.observe("order").unwrap().samples, vec![1.0, 0.0]);
    assert_eq!(r.observe("done").unwrap().samples, vec![4.0, 13.0]);
    // and the opposite order is a program too
    let r = run(&prog("serve by (-remaining);"), &common::horizon(100.0));
    assert_eq!(r.observe("order").unwrap().samples, vec![0.0, 1.0]);
    assert_eq!(r.observe("done").unwrap().samples, vec![10.0, 13.0]);
    // `decode first` and its expansion are the same program
    let ir = |s: &str| {
        serq::compile_source(&common::main_source(&prog(s)), &common::horizon(100.0))
            .unwrap()
            .to_json()
    };
    assert_eq!(
        ir("serve decode first;"),
        ir("serve by (decoding ? 0 : 1);")
    );
    // a serve key is read at its own moment only
    let e = serq::compile_source(
        &common::main_source(&prog("serve by (tokens);")),
        &common::horizon(100.0),
    )
    .unwrap_err();
    assert!(
        e.contains("`tokens` is read in a step stage's serve keys"),
        "{e}"
    );
}

/// A resident admitted in the middle of an iteration (with the budget the
/// residents left) may sort ahead of residents already served under `serve
/// by`. Each resident is served once: the newcomer gets the budget left,
/// nobody is served twice, nobody is skipped. Budget 2: at step 2 A's
/// decode (3 left) takes one token, B is admitted with the other and sorts
/// first by `remaining` (1 < 3); B prefills at 2 and, with one output, is
/// done at 2; A decodes at 2, 3, 4. An index into a re-sorted list would
/// have given A both tokens at step 2 and B nothing.
#[test]
fn a_resident_admitted_mid_iteration_is_served_once_under_serve_by() {
    let src = r#"
        pool reqs { cap 8; admit via engine; }
        pool kv { cap 1e5; }
        stage engine : step { budget 2; cost 1; memory kv; serve by (remaining); }
        stage gate : delay;
        workload { arrive batch(2); init { set arrive = serial; set o = serial == 0 ? 4 : 1; }
          session { request;
            end;

          }
        }
        server {
          run gate (arrive);
          hold reqs (1), kv (10) {
            run engine prefill (1) growing kv;
            run engine decode (o - 1) growing kv;
          }
          observe done = now;
          observe order = serial;
        }

"#;
    let r = run(src, &common::horizon(50.0));
    assert_eq!(
        r.observe("order").unwrap().samples,
        vec![1.0, 0.0],
        "{}",
        r.text()
    );
    assert_eq!(
        r.observe("done").unwrap().samples,
        vec![2.0, 4.0],
        "{}",
        r.text()
    );
}

/// scheduler.py:872-884, 1228-1235: FCFS with head-of-line blocking; a
/// request that does not fit stops the waiting loop even if a later,
/// smaller one would fit.
#[test]
fn admission_is_fcfs_with_head_of_line_blocking() {
    // blocks: 10 of 16 = 160 tokens. r0: 96 (6 blocks), r1: 96 (does not fit
    // with r0), r2: 16 (would fit). r2 must wait behind r1.
    let src = engine(3, "serial == 2 ? 16 : 96", "2", 10, 16, 1000, 16, "");
    let r = run(&src, &common::horizon(1000.0));
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
    let r = run(
        &engine(1, "3000", "1", 1000, 16, 1024, 16, ""),
        &common::horizon(1000.0),
    );
    let ttft = r.observe("ttft").unwrap().samples[0];
    assert_eq!(ttft, 3.0, "{}", r.text());
}

/// scheduler.py:606-616, 675-676: `long_prefill_token_threshold` caps one
/// request's chunk only when it is not alone. The program says so in its
/// `chunk` (`lib/vllm.sq`'s `long_prefill`); a constant cap is not vLLM's.
#[test]
fn long_prefill_threshold_applies_only_with_company() {
    let chunk = "chunk (residents + queued(reqs) > 1 ? 1000 : 0);";
    // alone: uncapped, the whole 3000-token prompt in one 4096-token step
    let alone = run(
        &engine(1, "3000", "1", 1000, 16, 4096, 16, chunk),
        &common::horizon(1000.0),
    );
    assert_eq!(
        alone.observe("ttft").unwrap().samples,
        vec![1.0],
        "{}",
        alone.text()
    );
    // with company: 1000 tokens each per step, three steps
    let two = run(
        &engine(2, "3000", "1", 1000, 16, 4096, 16, chunk),
        &common::horizon(1000.0),
    );
    assert_eq!(
        two.observe("ttft").unwrap().samples,
        vec![3.0, 3.0],
        "{}",
        two.text()
    );
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
        workload { arrive batch(1); init { set K = 0; set turns = 0; }
          session {
            loop { request;
              branch (turns >= 3) { end; }
            }

          }
        }
        server {
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
        }

"#;
    let r = run(src, &common::horizon(1000.0));
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
        workload { arrive batch(2); init { set K = 0; set turns = 0; }
          session {
            // session 0 runs first (160 tokens -> 10 blocks cached), then session 1
            // takes 12 blocks, evicting 2 of session 0's from its tail; session 0's
            // second turn then reuses 8 blocks.
            run gate (serial * 2);
            loop { request;
              branch (turns >= 2 || serial == 1) { end; }
              run gate (10);
            }

          }
        }
        server {
          set prompt = serial == 0 ? K + 160 : 192;
          hold reqs (1), kv (c + min(prompt - c, 8192))
          at admission (c = min(cachedin(kv), floor((prompt - 1) / bs) * bs)) {
            branch (serial == 0) { observe cached0 = cached; }
            run engine prefill (prompt - min(cached, floor((prompt - 1) / bs) * bs)) growing kv;
          } cache (prompt);
          set K = prompt;
          set turns = turns + 1;
        }

"#;
    let r = run(src, &common::horizon(1000.0));
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
    let done_of = |r: &serq::Report, who: f64| -> f64 {
        let order = &r.observe("order").unwrap().samples;
        let done = &r.observe("done").unwrap().samples;
        done[order.iter().position(|&s| s == who).unwrap()]
    };
    let shared = run(
        &engine(
            2,
            "serial == 0 ? 1 : 2048",
            "serial == 0 ? 10 : 1",
            1000,
            16,
            1024,
            16,
            "",
        ),
        &common::horizon(1000.0),
    );
    let excl = run(
        &engine(
            2,
            "serial == 0 ? 1 : 2048",
            "serial == 0 ? 10 : 1",
            1000,
            16,
            1024,
            16,
            "serve exclusive prefill;",
        ),
        &common::horizon(1000.0),
    );
    // request 0: 1 prefill step + 9 decode steps = 10 when sharing; with
    // exclusive prefill it waits for request 1's two chunks: 12
    assert_eq!(done_of(&shared, 0.0), 10.0, "{}", shared.text());
    assert_eq!(done_of(&excl, 0.0), 12.0, "{}", excl.text());
}

/// scheduler.py:877-879: `max_num_seqs` caps the running set.
#[test]
fn request_cap_is_a_slot_pool() {
    let r = run(
        &engine(4, "16", "5", 1000, 16, 8192, 2, ""),
        &common::horizon(1000.0),
    );
    let adm = &r.observe("admitted").unwrap().samples;
    let mut a = adm.clone();
    a.sort_by(f64::total_cmp);
    assert_eq!(a, vec![0.0, 0.0, 5.0, 5.0], "{}", r.text());
}
