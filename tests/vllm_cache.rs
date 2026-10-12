//! The multi-turn prefix-cache scenario (`tools/oracle/cache_trace.*`).
//!
//! Its IR, `tools/oracle/cache_trace.ir.json`, is the vLLM replay program
//! (`examples/replay/vllm_replay.sq`) on a unit step clock with the scenario's
//! engine, and the trace `cache_trace.csv` inlined as explicit sessions
//! with turns: the IR carries its whole workload. Run, it gives for every
//! turn the first-token step, the last-token step and the cached tokens
//! that the real scheduler gives (`cache_trace.out.csv`, from
//! `tools/vllm_replay_oracle.py`). The Lean theorem `vllm_cache_trace`
//! of serving-queue-theory is generated from the same IR file.
//! `SERQ_BLESS=1` rewrites the IR file (`make oracle-ir`).

mod common;

use std::path::Path;

use serq::frontend::parser;
use serq::{Overrides, Program, inline_trace, program_path, run_ir};

fn dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/oracle")
}

/// The scenario's IR, built from the program and the trace.
fn cache_ir() -> Program {
    let mut ov = Overrides {
        trace: Some(dir().join("cache_trace.csv").display().to_string()),
        ..common::horizon(6000.0)
    };
    for (k, v) in [
        ("N", "3"),
        ("spacing", "3"),
        ("B", "64"),
        ("max_seqs", "4"),
        ("blocks", "20"),
        ("c_it", "1"),
        ("d", "0"),
        ("e", "0"),
        ("a", "0"),
        ("b", "0"),
        ("c0", "0"),
        ("f", "0"),
    ] {
        ov.lets
            .push((k.to_string(), parser::parse_expr(v).unwrap()));
    }
    let src = std::fs::read_to_string(program_path("vllm_replay")).unwrap();
    let p = serq::compile_source(&common::main_source(&src), &ov).unwrap();
    inline_trace(p, None).unwrap()
}

#[test]
fn cache_ir_file_is_current() {
    let path = dir().join("cache_trace.ir.json");
    let want = cache_ir().to_json() + "\n";
    if std::env::var_os("SERQ_BLESS").is_some() {
        std::fs::write(&path, &want).unwrap();
    } else {
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            have == want,
            "{} is stale: run `make oracle-ir`",
            path.display()
        );
    }
}

#[test]
fn cache_trace_matches_the_real_scheduler() {
    let ir =
        Program::from_json(&std::fs::read_to_string(dir().join("cache_trace.ir.json")).unwrap())
            .unwrap();
    assert!(ir.trace.is_none(), "the IR carries its workload");
    let r = run_ir(&ir, None).unwrap();
    let get = |name: &str| {
        let o = r.observe(name).unwrap();
        let mut m = std::collections::HashMap::new();
        for (rec, v) in o.records.iter().zip(&o.samples) {
            m.insert((rec.1, rec.2), *v);
        }
        m
    };
    let (sent, ttft, lat, cached) = (
        get("sent"),
        get("ttft"),
        get("response"),
        get("cached_tokens"),
    );
    let want = std::fs::read_to_string(dir().join("cache_trace.out.csv")).unwrap();
    let mut n = 0;
    for line in want.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        let key = (f[0].parse::<u64>().unwrap(), f[1].parse::<u32>().unwrap());
        let (first, done, c): (f64, f64, f64) = (
            f[3].parse().unwrap(),
            f[4].parse().unwrap(),
            f[6].parse().unwrap(),
        );
        assert_eq!(sent[&key] + ttft[&key], first, "{key:?} first token");
        assert_eq!(sent[&key] + lat[&key], done, "{key:?} last token");
        assert_eq!(cached[&key], c, "{key:?} cached tokens");
        n += 1;
    }
    assert_eq!(n, 9);
}

/// The shared-system-prompt scenario (`tools/oracle/shared_trace.*`, written
/// by `tools/oracle/shared_trace.py`): three sessions of three turns whose
/// prompts all begin with one system prompt of 64 tokens, replayed by the
/// real scheduler and KV-cache manager (`tools/vllm_replay_oracle.py`, the
/// command in `shared_trace.py`) and by `examples/oracle/vllm_shared_prompt.sq`
/// (#374). Every turn's first and last token and cached tokens agree: the
/// later sessions' first turns read the system prompt another session
/// computed, and eviction takes the sessions' own tails before it.
#[test]
fn shared_trace_matches_the_real_scheduler() {
    let ov = Overrides {
        trace: Some(dir().join("shared_trace.csv").display().to_string()),
        ..common::horizon(200.0)
    };
    let src = std::fs::read_to_string(program_path("vllm_shared_prompt")).unwrap();
    let p = serq::compile_source(&common::main_source(&src), &ov).unwrap();
    let r = run_ir(&inline_trace(p, None).unwrap(), None).unwrap();
    let get = |name: &str| {
        let o = r.observe(name).unwrap();
        let mut m = std::collections::HashMap::new();
        for (rec, v) in o.records.iter().zip(&o.samples) {
            m.insert((rec.1, rec.2), *v);
        }
        m
    };
    let (sent, ttft, lat, cached) = (
        get("sent"),
        get("ttft"),
        get("response"),
        get("cached_tokens"),
    );
    let want = std::fs::read_to_string(dir().join("shared_trace.out.csv")).unwrap();
    let (mut n, mut shared) = (0, 0);
    for line in want.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        let key = (f[0].parse::<u64>().unwrap(), f[1].parse::<u32>().unwrap());
        let (first, done, c): (f64, f64, f64) = (
            f[3].parse().unwrap(),
            f[4].parse().unwrap(),
            f[6].parse().unwrap(),
        );
        assert_eq!(sent[&key] + ttft[&key], first, "{key:?} first token");
        assert_eq!(sent[&key] + lat[&key], done, "{key:?} last token");
        assert_eq!(cached[&key], c, "{key:?} cached tokens");
        if key.0 > 0 && key.1 == 1 && c > 0.0 {
            shared += 1;
        }
        n += 1;
    }
    assert_eq!(n, 9);
    assert_eq!(
        shared, 2,
        "the later sessions' first turns read the system prompt"
    );
}
