//! The P/D prefiller as an engine request
//! (`docs/design/request-state.md`): a KV relation's source registers a send
//! of each request's blocks at its end, which keeps them as vLLM's NIXL
//! connector does (`delay_free_blocks`) until the decoder's load has read
//! them, or the relation's `ttl`.

use std::path::{Path, PathBuf};

mod common;

/// An example's deployment, and the workload file `examples/pairs.json`
/// pairs it with.
fn example(path: &str) -> (String, PathBuf) {
    let workload = serq::paired_workload(Path::new(path)).expect("a pair");
    (std::fs::read_to_string(path).unwrap(), workload)
}

/// The report of a deployment under the workload file `workload`, seed 1,
/// without the lines of the observations `but`, nor its requests' statuses
/// (a hold is no engine request).
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
    let text = r.text();
    let mut status = false;
    text.lines()
        .filter(|l| {
            status = (status || l.starts_with("status ")) && !l.is_empty();
            !status
        })
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
          cache (request.input);
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
/// hold did not keep. The send is what the decoder reads: a prefiller that
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

/// llm-d's example with the relation's `ttl` written `t`.
fn with_ttl(t: &str) -> String {
    let (src, _) = example("examples/pd-disaggregation/llmd_nixl_pull.sq");
    let rel = "kvt.nixl_pull (P, D, latency = x0, share = share.maxmin);";
    assert!(src.contains(rel));
    src.replace(
        rel,
        &format!("kvt.nixl_pull (P, D, latency = x0, share = share.maxmin, ttl = {t});"),
    )
}

/// An unread send ends at the relation's `ttl` (vLLM's lease on a finished
/// request's blocks, `_kv_lease_duration`, nixl/pull_scheduler.py:248-262):
/// `inf`, the default, is the program as written; a `ttl` shorter than the
/// reads frees the prefiller's blocks before some are read, and a read of a
/// send gone reads nothing there.
#[test]
fn an_unread_send_ends_at_the_relation_s_ttl() {
    let (src, workload) = example("examples/pd-disaggregation/llmd_nixl_pull.sq");
    assert_eq!(
        report(&with_ttl("inf"), &workload, &[]),
        report(&src, &workload, &[])
    );
    let w = std::fs::read_to_string(&workload).unwrap();
    let ov = serq::Overrides {
        horizon: Some(300.0),
        seed: Some(1),
        ..Default::default()
    };
    let r = serq::run_paired(
        &with_ttl("0.005"),
        &w,
        common::NAMES,
        &ov,
        workload.parent(),
    )
    .unwrap();
    let fin = r.statuses.iter().find(|x| x.name == "finished").unwrap();
    assert!(fin.stays > 0);
    // within the quantile's 0.5 %
    assert!(fin.p99 <= 0.005 * 1.005, "{}", r.text());
}

/// A `ttl` is a positive constant: a setting of the relation, read once.
#[test]
fn a_ttl_is_a_positive_constant() {
    let (_, workload) = example("examples/pd-disaggregation/llmd_nixl_pull.sq");
    let w = std::fs::read_to_string(&workload).unwrap();
    for t in ["0", "-1", "now"] {
        let e = match serq::run_paired(
            &with_ttl(t),
            &w,
            common::NAMES,
            &common::horizon(10.0),
            workload.parent(),
        ) {
            Err(e) => e,
            Ok(_) => panic!("accepted ttl = {t}"),
        };
        assert!(e.contains("`ttl` is a number above 0"), "{e}");
    }
}
