//! Different workloads (`docs/use-cases/workloads.md`).
//!
//! The use case's claim is that the single-turn, chat and subagent programs
//! run `examples/multi-turn/vllm.sq`'s engine and differ from it only in the client.
//! The engine is the text a reader would compare: the constants from `B` to
//! `c0`, the declarations from `device gpu` to `pool reqs on vllm`, among
//! them the `engine vllm` block with its `request`, and the `server` that
//! calls it. Held to the text, not the IR, because the request is spliced
//! into the session and the IR has no engine to compare. Snippet markers
//! (`// --8<-- [start:…]`) are not text a reader sees, so they are skipped.

use serq::program_path;

fn engine(name: &str) -> Vec<String> {
    engine_of(&program_path(name), name)
}

fn engine_of(path: &std::path::Path, name: &str) -> Vec<String> {
    let src = std::fs::read_to_string(path).unwrap();
    let lines: Vec<&str> = src
        .lines()
        .filter(|l| !l.contains("--8<--"))
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
    out.extend(span("server (", &|l| l == "}"));
    out
}

#[test]
fn every_workload_runs_the_vllm_engine() {
    let vllm = engine("vllm");
    for name in ["vllm_single_turn", "vllm_chat", "vllm_subagents"] {
        assert_eq!(engine(name), vllm, "{name}'s engine is not vllm.sq's");
    }
    // the deployment the paired workloads run on (docs/design/paired-workload.md)
    let paired = std::path::Path::new("examples/paired/deployments/vllm.sq");
    assert_eq!(
        engine_of(paired, "paired vllm"),
        vllm,
        "the paired deployment's engine is not vllm.sq's"
    );
}
