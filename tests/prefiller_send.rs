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

const LLMD: &str = "examples/pd-disaggregation/llmd_nixl_pull.sq";

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

fn compiled(src: &str, workload: &Path, ov: &serq::Overrides) -> Result<serq::Report, String> {
    let w = std::fs::read_to_string(workload).unwrap();
    serq::run_paired(src, &w, common::NAMES, ov, workload.parent())
}

/// A source read by a connector is an engine whose requests are sent: the
/// prefiller written as a hold that leases, the other spelling of the same
/// prefill, does not link, and the message names the one spelling (one
/// meaning, one spelling; its `ttl` would not have reached the lease).
#[test]
fn a_hold_that_leases_is_no_source_for_a_connector() {
    let (src, workload) = example(LLMD);
    assert!(src.contains(REQUEST) && src.contains(ENTRY));
    let hold = src
        .replace(REQUEST, HOLD_SCHEDULER)
        .replace(ENTRY, HOLD_ENTRY);
    let e = compiled(&hold, &workload, &common::horizon(10.0)).unwrap_err();
    assert!(
        e.contains("`P.prefill` holds and leases instead: write the entry as a call of its engine, `P (prompt, 1);`"),
        "{e}"
    );
}

/// llm-d's example with the relation's `ttl` written `t`.
fn with_ttl(t: &str) -> String {
    let (src, _) = example(LLMD);
    let rel = "kvt.nixl_pull (P, D, latency = x0, share = share.maxmin);";
    assert!(src.contains(rel));
    src.replace(
        rel,
        &format!("kvt.nixl_pull (P, D, latency = x0, share = share.maxmin, ttl = {t});"),
    )
}

/// A send nobody reads ends at the relation's `ttl` (vLLM's lease on a
/// finished request's blocks, `_kv_lease_duration`,
/// nixl/pull_scheduler.py:248-262), and a decoder that comes for it after
/// that reads nothing: it computes the prompt itself (vLLM recomputes a
/// read of invalid blocks, scheduler.py:3254-3257). With the decoders'
/// memory short (`blocksD = 1000`), requests wait for it; the shorter the
/// `ttl`, the less the prefillers hold, the more the decoders compute, and
/// the later the first token (PR #719's second review: a send expired was
/// still read in full).
#[test]
fn an_expired_send_is_computed_again() {
    let (_, workload) = example(LLMD);
    let mut ov = serq::Overrides {
        horizon: Some(300.0),
        seed: Some(1),
        ..Default::default()
    };
    ov.set("blocksD", "1000").unwrap();
    let run = |src: &str| compiled(src, &workload, &ov).unwrap();
    let (src, _) = example(LLMD);
    let kept = run(&src);
    let short = run(&with_ttl("0.002"));
    let mean = |r: &serq::Report, o: &str| r.observe(o).unwrap().mean;
    let pkv = |r: &serq::Report| -> f64 { r.pools_named("P.kv").iter().map(|p| p.mean_used).sum() };
    assert!(mean(&short, "D.prefill") > 2.0 * mean(&kept, "D.prefill"));
    assert!(mean(&short, "D.loaded") < mean(&kept, "D.loaded"));
    assert!(short.client("ttft").unwrap().mean > kept.client("ttft").unwrap().mean);
    assert!(pkv(&short) < pkv(&kept));
}

/// A `ttl` is a positive constant: a setting of the relation, read once.
/// `inf` is the default, left out (one meaning, one spelling).
#[test]
fn a_ttl_is_a_positive_constant() {
    let (_, workload) = example(LLMD);
    for (t, why) in [
        ("0", "`ttl` is a number above 0"),
        ("-1", "`ttl` is a number above 0"),
        ("now", "`ttl` is a number above 0"),
        ("inf", "`ttl = inf` is the default"),
    ] {
        let e = compiled(&with_ttl(t), &workload, &common::horizon(10.0)).unwrap_err();
        assert!(e.contains(why), "{t}: {e}");
    }
}
