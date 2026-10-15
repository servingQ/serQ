//! Different workloads (`docs/use-cases/workloads.md`).
//!
//! The use case's claim is that the single-turn, chat and subagent clients
//! run `examples/multi-turn/vllm.sq`'s engine and differ only in the client.
//! The engine is one file, so that holds by construction: what is left to
//! hold is that each of the use case's workloads is one `examples/pairs.json`
//! runs `vllm.sq` under, and that it compiles there.

use std::path::Path;

#[test]
fn every_workload_runs_the_vllm_engine() {
    let deployment = Path::new("examples/multi-turn/vllm.sq");
    let paired = serq::paired_workloads(deployment);
    for w in ["chat", "human_chat", "single_turn", "subagents"] {
        let w = Path::new("examples/workloads").join(format!("{w}.sq"));
        assert!(
            paired.iter().any(|p| p.ends_with(&w)),
            "{}: not among vllm.sq's workloads",
            w.display()
        );
        serq::load_paired(
            deployment,
            &w,
            &serq::Overrides {
                horizon: Some(10.0),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{}: {e}", w.display()));
    }
}
