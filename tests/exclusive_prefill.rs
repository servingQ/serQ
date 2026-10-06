//! Full-attention batch isolation, derived from the four guards in native
//! vllm-rbln v0.11.3a21's RBLNScheduler.schedule (lines 196-207, 252-259,
//! 440-451, 651-663, 863-890). We exercise the serQ mechanism, not the vendor
//! runtime or PP/remote-KV policies. Unit step costs make the schedule explicit.

mod common;

use serq::{Program, compile_source, run_ir, run_source};
use std::path::Path;
use std::process::Command;

fn source(policy: &str, slot_cap: usize, kv_cap: usize) -> String {
    format!(
        r#"
        pool reqs {{ cap {slot_cap}; admit via engine; }}
        pool kv {{ cap {kv_cap}; }}
        stage gate : delay;
        stage engine : step {{ budget 4; chunk 4; cost 1; memory kv; {policy} }}
        workload {{ arrive batch(2); init {{ set prompt = serial == 0 ? 2 : 4; }}
          session {{ request;
            end;

          }}
        }}
        server {{
          run gate (cost(gate, serial));
          hold reqs (cost(reqs, 1)), kv (cost(kv, min(prompt, left))) reserve (cost(kv, prompt))
          at admission (left = budget_left(engine)) {{
            observe allocation = used(kv);
            run engine prefill (cost(engine, prompt)) growing kv;
            observe prefill_done = now;
            observe prefill_who = serial;
            branch (serial == 0) {{ run engine decode (cost(engine, 2)) growing kv; }}
          }} cache (cost(reqs, kv, 100));
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
    let src = source("serve exclusive prefill;", 2, 20);
    assert_eq!(
        trace("takeover", &src),
        [
            "ITER 0.0000 0:0:p2",
            "ITER 1.0000 1:0:p4",
            "ITER 2.0000 0:0:d1",
            "ITER 3.0000 0:0:d1",
        ]
    );
    let r = run_source(&common::main_source(&src), &common::horizon(20.0), None).unwrap();
    let json = compile_source(&common::main_source(&src), &common::horizon(20.0))
        .unwrap()
        .to_json();
    let from_ir = run_ir(&Program::from_json(&json).unwrap(), None);
    assert_eq!(r.json(), from_ir.unwrap().json());
    assert_eq!(r.observe("allocation").unwrap().samples, [2.0, 7.0]);
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
        trace("mixed", &source("", 2, 20)),
        [
            "ITER 0.0000 0:0:p2",
            "ITER 1.0000 0:0:d1 1:0:p3",
            "ITER 2.0000 0:0:d1 1:0:p1",
        ]
    );
}

#[test]
fn a_waiting_prefill_that_does_not_fit_keeps_the_decode_batch() {
    // One slot: B cannot enter until A finishes at t=3, so A's decode
    // batches stay at t=1 and t=2 and B prefills alone at t=3.
    assert_eq!(
        trace("slots", &source("serve exclusive prefill;", 1, 20)),
        [
            "ITER 0.0000 0:0:p2",
            "ITER 1.0000 0:0:d1",
            "ITER 2.0000 0:0:d1",
            "ITER 3.0000 1:0:p4",
        ]
    );
    // Four KV units: A's growing decode holds 3 at t=1 and 4 at t=2,
    // leaving too little for B's full-sequence gate; same schedule.
    assert_eq!(
        trace("memory", &source("serve exclusive prefill;", 2, 4)),
        [
            "ITER 0.0000 0:0:p2",
            "ITER 1.0000 0:0:d1",
            "ITER 2.0000 0:0:d1",
            "ITER 3.0000 1:0:p4",
        ]
    );
}

#[test]
fn resident_prefill_chunks_do_not_admit_another_waiting_request() {
    // A lone prefill uses two chunks (4,2). Only when those finish can the
    // next waiting prefill displace the tentative decode. Thus three fresh
    // requests enter at 0,2,4, not all during the first prefill's unused budget.
    let src = r#"
        pool reqs { cap 3; admit via engine; }
        pool kv { cap 40; }
        stage engine : step {
          budget 4; chunk 4; cost 1; memory kv; serve exclusive prefill;
        }
        workload { arrive batch(3);
          session { request;
            end;

          }
        }
        server {
          hold reqs (cost(reqs, 1)), kv (cost(kv, 4)) reserve (cost(kv, 6)) {
            observe admitted = now;
            run engine prefill (cost(engine, 6)) growing kv;
            run engine decode (cost(engine, 1)) growing kv;
          }
        }

"#;
    let r = run_source(&common::main_source(src), &common::horizon(20.0), None).unwrap();
    assert_eq!(r.ended, 3);
    assert_eq!(r.observe("admitted").unwrap().samples, [0.0, 2.0, 4.0]);
    assert_eq!(r.stage("engine").unwrap().iterations, 7);
}

#[test]
fn an_exhausted_decode_budget_defers_waiting_prefill() {
    // A and B each decode two tokens. Budget 2 is exhausted by their
    // resident decode batch, so waiting C cannot take over: admission is
    // still gated by positive remaining budget, not just full-budget fit.
    // C enters at 2 and prefills in two chunks, finishing at 4.
    let src = r#"
        pool reqs { cap 3; admit via engine; }
        stage engine : step { budget 2; cost 1; serve exclusive prefill; }
        workload { arrive batch(3);
          session { request;
            end;

          }
        }
        server {
          hold reqs (cost(reqs, 1)) {
            observe admitted = now;
            branch (serial < 2) {
              run engine decode (cost(engine, 2));
            } else {
              run engine prefill (cost(engine, 4));
            }
          }
          observe done = now;
        }

"#;
    let r = run_source(&common::main_source(src), &common::horizon(10.0), None).unwrap();
    assert_eq!(r.ended, 3);
    assert_eq!(r.observe("admitted").unwrap().samples, [0.0, 0.0, 2.0]);
    assert_eq!(r.observe("done").unwrap().samples, [2.0, 2.0, 4.0]);
}

#[test]
fn preemption_keeps_only_committed_progress_and_defers_readmission() {
    // Six KV units, two 2-token prompts, three decode tokens each:
    // t0 A:p2; t1 B:p2 displaces A's tentative decode (A holds 3, knows 2);
    // t2 A:d1+B:d1 uses all six units; t3 A's growth preempts B at known=3.
    // No waiting admission follows that preemption; t4 A:d1 ends at 5.
    // B enters at 5 with known=3, recomputes in chunks 2,1, then decodes the
    // remaining two tokens at t7,t8. Both final computed extents are 5.
    // `reuse (0)` removes hits from this arithmetic; the requested cache
    // bound is deliberately loose to expose any unexecuted KV publication.
    let src = r#"
        pool reqs { cap 2; admit via engine; }
        pool kv { cap 6; preempt lifo; }
        stage engine : step {
          budget 4; chunk 2; cost 1; memory kv; serve exclusive prefill;
        }
        workload { arrive batch(2);
          session { request;
            end;

          }
        }
        server {
          hold reqs (cost(reqs, 1)), kv (cost(kv, min(known, left))) reserve (cost(kv, known)) reuse (cost(reqs, kv, 0))
          at admission (known = max(2, computed), left = budget_left(engine)) {
            branch (serial == 1) {
              observe admitted_b = now;
              observe restored_b = known;
            }
            run engine prefill (cost(engine, known)) growing kv;
            run engine decode (cost(engine, 3 - (known - 2))) growing kv;
          } cache (cost(reqs, kv, 100));
          observe cached_extent = cachedin(kv);
          observe done = now;
        }

"#;
    let r = run_source(&common::main_source(src), &common::horizon(20.0), None).unwrap();
    assert_eq!(r.ended, 2, "{}", r.text());
    assert_eq!(r.pool("kv").unwrap().preemptions, 1);
    assert_eq!(r.observe("admitted_b").unwrap().samples, [1.0, 5.0]);
    assert_eq!(r.observe("restored_b").unwrap().samples, [2.0, 3.0]);
    assert_eq!(r.observe("done").unwrap().samples, [5.0, 9.0]);
    assert_eq!(r.observe("cached_extent").unwrap().samples, [5.0, 5.0]);
}
