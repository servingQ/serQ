//! Full-attention batch isolation, derived from the four guards in native
//! vllm-rbln v0.11.3a21's RBLNScheduler.schedule (lines 196-207, 252-259,
//! 440-451, 651-663, 863-890). We exercise the serQ mechanism, not the vendor
//! runtime or PP/remote-KV policies. Unit step costs make the schedule explicit.

mod common;

use serq::{Program, compile_source, run_ir, run_source};
use std::path::Path;
use std::process::Command;

/// The schedule that takes a waiting prefill alone, in chunks of 4: the
/// stage's `serve exclusive prefill`.
const EXCLUSIVE: &str = "exclusive prefill each at most (4); admit waiting while (running.preempted == 0) each at most (4) taking reqs (1), kv (min(request.input, budget_left(llm))) reserve (request.input) cache (100);";

/// The default schedule, in chunks of 4.
const MIXED: &str = "advance running each at most (4); admit waiting while (running.preempted == 0) each at most (4) taking reqs (1), kv (min(request.input, budget_left(llm))) reserve (request.input) cache (100);";

fn source(schedule: &str, slot_cap: usize, kv_cap: usize) -> String {
    format!(
        r#"
        stage gate : delay;
        device gpu {{ kv cap {kv_cap}; }}
        engine llm on gpu {{
          reqs cap {slot_cap};
          tokens cap 4;
          scheduler {{ {schedule} }}
          worker {{ forward (1); }}
        }}
        pool reqs on llm {{ }}
        pool kv on gpu {{ }}
        workload {{ arrive batch(2); init {{ set prompt = serial == 0 ? 2 : 4; }}
          session {{ turn;
            end;

          }}
        }}
        server {{
          run gate (cost(gate, serial));
          // A decodes two tokens after its prefill's; B only prefills
          llm (prompt, serial == 0 ? 3 : 1);
          observe final_cached = cachedin(kv);
          observe done = now;
          observe who = serial;
        }}

"#
    )
}

fn trace(name: &str, src: &str) -> Vec<String> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("exclusive-prefill");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.sq"));
    std::fs::write(&path, common::main_source(src)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_serq"))
        .env("SERQ_TRACE_ITER", "1")
        .args(["run", path.to_str().unwrap(), "--horizon", "20", "--json"])
        .output()
        .unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(out.status.success(), "{stderr}");
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["arrivals"], 2);
    assert_eq!(report["ended"], 2);
    stderr
        .lines()
        .filter(|l| l.starts_with("ITER "))
        .map(|l| l.split(" | ").next().unwrap().to_owned())
        .collect()
}

#[test]
fn waiting_prefill_replaces_decodes_and_gets_the_full_budget() {
    // At t=0 A prefills 2 tokens. B arrives at t=1: A's tentative decode
    // is displaced by B's lone 4-token prefill (not clipped to 3). A then
    // decodes at t=2 and t=3. This is guards (B)/(D); B finishes at 2, A at 4.
    let src = source(EXCLUSIVE, 2, 20);
    assert_eq!(
        trace("takeover", &src),
        [
            "ITER 0.0000 0:1:p2",
            "ITER 1.0000 1:1:p4",
            "ITER 2.0000 0:1:d1",
            "ITER 3.0000 0:1:d1",
        ]
    );
    let r = run_source(&common::main_source(&src), &common::horizon(20.0), None).unwrap();
    let json = compile_source(&common::main_source(&src), &common::horizon(20.0))
        .unwrap()
        .to_json();
    let from_ir = run_ir(&Program::from_json(&json).unwrap(), None);
    assert_eq!(r.json(), from_ir.unwrap().json());
    assert_eq!(r.observe("who").unwrap().samples, [1.0, 0.0]);
    assert_eq!(r.observe("done").unwrap().samples, [2.0, 4.0]);
    // The cancelled A decode reserved one token of capacity but computed
    // none. Only 2+2 tokens for A and 4 for B may enter the finished cache.
    assert_eq!(r.observe("final_cached").unwrap().samples, [4.0, 4.0]);
}

#[test]
fn mixed_batching_remains_the_default() {
    // The same arrivals with ordinary resident-first selection mix A's
    // decode with the first 3 B tokens, then its decode with B's last token.
    assert_eq!(
        trace("mixed", &source(MIXED, 2, 20)),
        [
            "ITER 0.0000 0:1:p2",
            "ITER 1.0000 0:1:d1 1:1:p3",
            "ITER 2.0000 0:1:d1 1:1:p1",
        ]
    );
}

#[test]
fn a_waiting_prefill_that_does_not_fit_keeps_the_decode_batch() {
    // One slot: B cannot enter until A finishes at t=3, so A's decode
    // batches stay at t=1 and t=2 and B prefills alone at t=3.
    assert_eq!(
        trace("slots", &source(EXCLUSIVE, 1, 20)),
        [
            "ITER 0.0000 0:1:p2",
            "ITER 1.0000 0:1:d1",
            "ITER 2.0000 0:1:d1",
            "ITER 3.0000 1:1:p4",
        ]
    );
    // Four KV units: A's growing decode holds 3 at t=1 and 4 at t=2,
    // leaving too little for B's full-sequence gate; same schedule.
    assert_eq!(
        trace("memory", &source(EXCLUSIVE, 2, 4)),
        [
            "ITER 0.0000 0:1:p2",
            "ITER 1.0000 0:1:d1",
            "ITER 2.0000 0:1:d1",
            "ITER 3.0000 1:1:p4",
        ]
    );
}

#[test]
fn resident_prefill_chunks_do_not_admit_another_waiting_request() {
    // A lone prefill uses two chunks (4,2). Only when those finish can the
    // next waiting prefill displace the tentative decode. Thus three fresh
    // requests enter at 0,2,4, not all during the first prefill's unused budget.
    let src = r#"
        device gpu { kv cap 40; }
        engine llm on gpu {
          reqs cap 3;
          tokens cap 4;
          scheduler {
            exclusive prefill each at most (4);
            admit waiting while (running.preempted == 0) each at most (4)
              taking reqs (1), kv (4) reserve (6);
          }
          worker { forward (1); }
        }
        pool reqs on llm { }
        pool kv on gpu { }
        workload { arrive batch(3);
          session { turn;
            end;

          }
        }
        server { llm (6, 2); observe admitted_at = llm.admitted; }

"#;
    let r = run_source(&common::main_source(src), &common::horizon(20.0), None).unwrap();
    assert_eq!(r.ended, 3);
    assert_eq!(r.observe("admitted_at").unwrap().samples, [0.0, 2.0, 4.0]);
    assert_eq!(r.stage("llm").unwrap().iterations, 7);
}

#[test]
fn an_exhausted_decode_budget_defers_waiting_prefill() {
    // A and B each decode two tokens. Budget 2 is exhausted by their
    // resident decode batch, so waiting C cannot take over: admission is
    // still gated by positive remaining budget, not just full-budget fit.
    // C enters at 2 and prefills in two chunks, finishing at 4.
    let src = r#"
        device gpu { }
        engine llm on gpu {
          reqs cap 3;
          tokens cap 2;
          scheduler { exclusive prefill; admit waiting while (running.preempted == 0) taking reqs (1); }
          worker { forward (1); }
        }
        pool reqs on llm { }
        workload { arrive batch(3);
          session { turn;
            end;

          }
        }
        server {
          // A and B decode two tokens with no prefill, C prefills four
          llm (serial < 2 ? 0 : 4, serial < 2 ? 2 : 1);
          observe admitted_at = llm.admitted;
          observe done = now;
        }

"#;
    let r = run_source(&common::main_source(src), &common::horizon(10.0), None).unwrap();
    assert_eq!(r.ended, 3);
    assert_eq!(r.observe("admitted_at").unwrap().samples, [0.0, 0.0, 2.0]);
    assert_eq!(r.observe("done").unwrap().samples, [2.0, 2.0, 4.0]);
}

#[test]
fn preemption_keeps_only_committed_progress_and_defers_readmission() {
    // Six KV units, two 2-token prompts, four output tokens each (the
    // prefill's and three decodes): t0 A:p2; t1 B:p2 displaces A's tentative
    // decode (A holds 3); t2 A:d1+B:d1 uses all six units; t3 A's growth
    // preempts B at position 3, which keeps the token it sampled there
    // (known=4, as vLLM). No waiting admission follows that preemption; t4
    // A:d1 ends at 5. B enters at 5 with known=4, recomputes in chunks 2,2,
    // then decodes its last token at t7. Both final computed extents are 5.
    // `reuse (0)` removes hits from this arithmetic; the requested cache
    // bound is deliberately loose to expose any unexecuted KV publication.
    let src = r#"
        device gpu { kv cap 6; }
        engine llm on gpu {
          reqs cap 2;
          tokens cap 4;
          scheduler {
            exclusive prefill each at most (2);
            admit waiting while (running.preempted == 0) each at most (2)
              taking reqs (1), kv (min(request.known, budget_left(llm))) reserve (request.known)
              reuse (0) cache (100);
          }
          worker { forward (1); }
        }
        pool reqs on llm { }
        pool kv on gpu { preempt lifo; }
        workload { arrive batch(2);
          session { turn;
            end;

          }
        }
        server {
          llm (2, 4);
          if (serial == 1) { observe admitted_b = llm.admitted; }
          observe cached_extent = cachedin(kv);
          observe done = now;
        }

"#;
    let r = run_source(&common::main_source(src), &common::horizon(20.0), None).unwrap();
    assert_eq!(r.ended, 2, "{}", r.text());
    assert_eq!(r.pool("kv").unwrap().preemptions, 1);
    // what each admission prefilled: A, B, then B again with its tokens
    assert_eq!(r.observe("llm.prefill").unwrap().samples, [2.0, 2.0, 4.0]);
    assert_eq!(r.observe("admitted_b").unwrap().samples, [5.0]);
    assert_eq!(r.observe("done").unwrap().samples, [5.0, 8.0]);
    assert_eq!(r.observe("cached_extent").unwrap().samples, [5.0, 5.0]);
}
