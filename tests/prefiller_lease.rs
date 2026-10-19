//! The P/D prefiller as an engine request
//! (`docs/design/prefiller-engine-request.md`): its `taking` keeps the
//! prompt's blocks past the request, `lease kv (inf)`, as vLLM's NIXL
//! connector keeps them (`delay_free_blocks`) until the decoder's connector
//! has read them and ends the lease.

use std::path::{Path, PathBuf};

mod common;

/// An example's deployment, and the workload file `examples/pairs.json`
/// pairs it with.
fn example(path: &str) -> (String, PathBuf) {
    let workload = serq::paired_workload(Path::new(path)).expect("a pair");
    (std::fs::read_to_string(path).unwrap(), workload)
}

/// The report of a deployment under the workload file `workload`, seed 1,
/// without the lines of the observations `but`.
fn report(src: &str, workload: &Path, but: &[&str]) -> String {
    let w = std::fs::read_to_string(workload).unwrap();
    let ov = serq::Overrides {
        horizon: Some(300.0),
        warmup: Some(30.0),
        seed: Some(1),
        ..Default::default()
    };
    let r = serq::run_paired(src, &w, common::NAMES, &ov, workload.parent())
        .unwrap_or_else(|e| panic!("{e}\n{src}"));
    r.text()
        .lines()
        .filter(|l| !but.iter().any(|b| l.starts_with(&format!("{b} "))))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `llmd_nixl_pull.sq`'s prefiller as an engine request.
const REQUEST: &str = "      scheduler {
        advance running;
        admit waiting while (running.preempted == 0)
          taking reqs (1),
                 kv (min(request.known, request.hit + budget_left(P))) reserve (request.known)
          reuse (floor(request.known - 1, bs))
          cache (request.input)
          lease kv (inf);
      }";

const ENTRY: &str = "    prefill (prompt) {
      P (prompt, 1);
    }";

/// The same prefiller as it was written before, its entry's hold.
const HOLD_SCHEDULER: &str =
    "      scheduler { advance running; admit waiting while (running.preempted == 0); }";

const HOLD_ENTRY: &str = "    prefill (prompt) {
      hold reqs (cost(reqs, 1)), kv (cost(kv, min(prompt, reused + budget_left(P)))) reserve (cost(kv, prompt))
           at admission (reused = min(cachedin(kv), floor(prompt - 1, bs))) {
        set c = cached;
        run P prefill (cost(P, prompt - c)) growing kv;
      } cache (cost(reqs, kv, prompt)) lease kv (inf);
    }";

/// The prefiller's request runs as its hold did: llm-d's example gives the
/// report line for line, split and with every request split (`thr = 0`),
/// but for the engine's records of the prefiller's admissions, which the
/// hold did not keep. The lease is what the decoder reads: a prefiller that
/// gave its blocks back at the request's end would let them be evicted.
#[test]
fn the_prefiller_s_request_runs_as_its_hold_did() {
    let (src, workload) = example("examples/pd-disaggregation/llmd_nixl_pull.sq");
    assert!(src.contains(REQUEST) && src.contains(ENTRY));
    let hold = src
        .replace(REQUEST, HOLD_SCHEDULER)
        .replace(ENTRY, HOLD_ENTRY);
    for thr in ["512", "0"] {
        let at = |s: &str| {
            s.replace(
                "args.number(\"thr\", 512)",
                &format!("args.number(\"thr\", {thr})"),
            )
        };
        let now = report(&at(&src), &workload, &["P.reused", "P.prefill"]);
        let before = report(&at(&hold), &workload, &[]);
        assert_eq!(now, before, "thr = {thr}");
    }
}

/// A prefiller engine whose `taking` is written `TAKING`, called by the
/// server; nothing reads its KV.
fn alone(taking: &str) -> String {
    format!(
        "queue P : prefill {{
           device gpu {{ kv cap 1000; }}
           engine on gpu {{
             reqs cap 4;
             tokens cap inf;
             scheduler {{ advance running; admit waiting while (running.preempted == 0) taking {taking}; }}
             worker {{ forward (1); }}
           }}
           pool reqs on P {{ }}
           pool kv on gpu {{ block 16; }}
           prefill (prompt) {{ P (prompt, 1); }}
         }}
         server {{ P.prefill (100); }}"
    )
}

fn refused(src: &str, needle: &str) {
    let e = match common::compile(
        &common::main_source(src),
        "workload { arrive batch(1); }",
        &common::horizon(100.0),
    ) {
        Err(e) => e,
        Ok(_) => panic!("accepted:\n{src}"),
    };
    assert!(e.contains(needle), "{src}\n  {e}");
}

/// A lease nothing ends would keep the blocks until the session's end,
/// read by nobody: the program does not link, and the message names what
/// ends one (CLAUDE.md, "Should the language have refused the program?").
#[test]
fn a_request_s_lease_nothing_reads_is_refused() {
    refused(
        &alone("reqs (1), kv (request.known) lease kv (inf)"),
        "no `release P.kv` ends it",
    );
}

/// The pool a request keeps is one its admission takes.
#[test]
fn a_request_leases_a_pool_it_takes() {
    refused(
        &alone("reqs (1) lease kv (inf)"),
        "engine `P`'s `lease kv`: its `taking` does not take `kv`",
    );
}
