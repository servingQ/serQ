//! Different workloads (`docs/use-cases/workloads.md`).
//!
//! The use case's claim is that the single-turn, chat and subagent programs
//! run `examples/multi-turn/vllm.sq`'s engine and differ from it only in the client.
//! The engine is the text a reader would compare: the constants from `B` to
//! `c0`, the declarations from `device gpu` to `pool reqs on vllm`, and the
//! `server` block. Held to the text, not the IR, because the server is spliced
//! into the session and the IR has no engine to compare.

use serq::program_path;

fn engine(name: &str) -> Vec<String> {
    let src = std::fs::read_to_string(program_path(name)).unwrap();
    let lines: Vec<&str> = src
        .lines()
        .map(|l| l.strip_prefix("  ").unwrap_or(l))
        .collect();
    let span = |from: &str, to: &dyn Fn(&str) -> bool| -> Vec<String> {
        let i = lines
            .iter()
            .position(|l| l.starts_with(from))
            .unwrap_or_else(|| panic!("{name}: no line starting `{from}`"));
        let j = i + lines[i..].iter().position(|l| to(l)).unwrap();
        lines[i..=j].iter().map(|l| l.to_string()).collect()
    };
    let mut out = span("let B ", &|l| l.starts_with("let c0 "));
    out.extend(span("device gpu ", &|l| {
        l.starts_with("pool reqs on vllm ")
    }));
    out.extend(span("server {", &|l| l == "}"));
    out
}

#[test]
fn every_workload_runs_the_vllm_engine() {
    let vllm = engine("vllm");
    for name in ["vllm_single_turn", "vllm_chat", "vllm_subagents"] {
        assert_eq!(engine(name), vllm, "{name}'s engine is not vllm.sq's");
    }
}
