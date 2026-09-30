//! Full-attention batch isolation, derived from the four guards in native
//! vllm-rbln v0.11.3a21's RBLNScheduler.schedule (lines 196-207, 252-259,
//! 440-451, 651-663, 863-890). We exercise the seQ mechanism, not the vendor
//! runtime or PP/remote-KV policies. Unit step costs make the schedule explicit.

use seq::{Overrides, Program, compile_source, run_ir, run_source};
use std::path::Path;
use std::process::Command;

fn source(policy: &str, slot_cap: usize, kv_cap: usize) -> String {
    format!(
        r#"
        pool reqs {{ cap {slot_cap}; admit via engine; }}
        pool kv {{ cap {kv_cap}; }}
        stage gate : delay;
        stage engine : step {{ budget 4; chunk 4; cost 1; memory kv; {policy} }}
        workload {{ arrive batch(2); init {{ set prompt = serial == 0 ? 2 : 4; }} }}
        session {{
          run gate (serial);
          hold reqs (1), kv (min(prompt, left)) reserve (prompt)
               at admission (left = budget_left(engine)) {{
            observe allocation = used(kv);
            run engine prefill (prompt) growing kv;
            observe prefill_done = now;
            observe prefill_who = serial;
            branch (serial == 0) {{ run engine decode (2) growing kv; }}
          }} cache (100);
          observe final_cached = cachedin(kv);
          observe done = now;
          observe who = serial;
          end;
        }}
        run {{ horizon 20; }}
        "#
    )
}

fn trace(name: &str, src: &str) -> Vec<String> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("exclusive-prefill");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.seq"));
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_seq-lang"))
        .env("SEQ_TRACE_ITER", "1")
        .args(["run", path.to_str().unwrap(), "--json"])
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
    let r = run_source(&src, &Overrides::default(), None).unwrap();
    let json = compile_source(&src, &Overrides::default())
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
        workload { arrive batch(3); }
        session {
          hold reqs (1), kv (4) reserve (6) {
            observe admitted = now;
            run engine prefill (6) growing kv;
            run engine decode (1) growing kv;
          }
          end;
        }
        run { horizon 20; }
    "#;
    let r = run_source(src, &Overrides::default(), None).unwrap();
    assert_eq!(r.ended, 3);
    assert_eq!(r.observe("admitted").unwrap().samples, [0.0, 2.0, 4.0]);
    assert_eq!(r.stage("engine").unwrap().iterations, 7);
}
