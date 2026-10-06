//! The programmable vLLM target against the real scheduler.
//!
//! `examples/oracle/vllm_request.sq` with a `serve by` order is a program
//! that vLLM's own scheduler does not run. `serq target` compiles it to
//! vLLM's configuration plus the programmable scheduler
//! (`tools/serq_vllm.py`, `scheduler_cls`) with the program's keys. Each
//! scenario of `tools/oracle/serve/` (the engine, the requests and the
//! order, `serve`) is compiled to IR (`<name>.ir.json`) and to its target
//! (`<name>.target.json`). The scheduler built from that target, driven by
//! `tools/vllm_oracle.py SCENARIO TARGET` as the oracle of
//! `tests/vllm_oracle.rs` is, answered `<name>.out.json`: the step of every
//! request's first and last token, and the preemptions. The interpreter
//! must give the same answer. `SERQ_BLESS=1` rewrites the IR and target
//! files; the answers come from the oracle run.
//!
//! The scenarios are outside the Lean fragment (it serves in admission
//! order), so they live apart from `tools/oracle/*.json`, which
//! `scripts/gen_lean_oracle.py` translates.

mod common;

use std::path::{Path, PathBuf};

use serde_json::Value;
use serq::{Overrides, compile_source_at, program_path, run_ir};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/oracle/serve")
}

fn num(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

fn read(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir().join(name)).unwrap()).unwrap()
}

const ENGINE: &str =
    "stage engine : step { budget B; chunk long_prefill(reqs, chunk); cost 1; memory kv; }";

/// The oracle program with the scenario's engine, order and requests.
fn ir(name: &str) -> serq::Program {
    let sc = read(&format!("{name}.json"));
    let mut ov = Overrides::default();
    for (k, v) in [
        ("bs", num(&sc, "block_size")),
        ("B", num(&sc, "budget")),
        ("blocks", num(&sc, "num_blocks") - 1.0), // the null block
        ("max_seqs", num(&sc, "max_seqs")),
        ("chunk", num(&sc, "chunk")),
    ] {
        ov.set_num(k, v).unwrap();
    }
    let src = std::fs::read_to_string(program_path("vllm_request")).unwrap();
    assert!(src.contains(ENGINE), "the oracle program's engine moved");
    let serve = sc["serve"].as_str().expect("a scenario names its order");
    let src = src.replace(
        ENGINE,
        &format!("stage engine : step {{ budget B; chunk long_prefill(reqs, chunk); cost 1; serve by ({serve}); memory kv; }}"),
    );
    let sessions: Vec<Vec<(&str, f64)>> = sc["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| {
            vec![
                ("prompt", num(q, "prompt")),
                ("o", num(q, "out")),
                ("arrive", num(q, "arrive")),
            ]
        })
        .collect();
    compile_source_at(
        &common::main_source(&src),
        program_path("vllm_request").parent(),
        &ov,
    )
    .unwrap()
    .with_sessions(&sessions)
    .unwrap()
}

fn scenarios() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir())
        .unwrap()
        .filter_map(|e| {
            let f = e.unwrap().file_name().into_string().unwrap();
            let n = f.strip_suffix(".json")?;
            (!n.contains('.')).then(|| n.to_string())
        })
        .collect();
    names.sort();
    assert!(!names.is_empty());
    names
}

#[test]
fn ir_and_target_files_are_current() {
    let bless = std::env::var_os("SERQ_BLESS").is_some();
    for n in scenarios() {
        let p = ir(&n);
        let target = serq::target::vllm(&p).unwrap_or_else(|e| panic!("{n}: {e}"));
        assert!(
            target["config"]["scheduler_cls"].is_string(),
            "{n}: the scenario's order is vLLM's own"
        );
        for (file, want) in [
            (format!("{n}.ir.json"), p.to_json() + "\n"),
            (
                format!("{n}.target.json"),
                serde_json::to_string_pretty(&target).unwrap() + "\n",
            ),
        ] {
            let path = dir().join(&file);
            if bless {
                std::fs::write(&path, &want).unwrap();
            } else {
                let have = std::fs::read_to_string(&path).unwrap_or_default();
                assert!(have == want, "{} is stale: SERQ_BLESS=1", path.display());
            }
        }
    }
}

#[test]
fn the_programmable_scheduler_decides_as_the_program_does() {
    for name in scenarios() {
        let Ok(text) = std::fs::read_to_string(dir().join(format!("{name}.out.json"))) else {
            panic!(
                "{name}: no oracle answer; run tools/vllm_oracle.py {name}.json {name}.target.json"
            );
        };
        let ans: Value = serde_json::from_str(&text).unwrap();
        let ir = serq::Program::from_json(
            &std::fs::read_to_string(dir().join(format!("{name}.ir.json"))).unwrap(),
        )
        .unwrap();
        let n = match &ir.arrival {
            serq::ir::CArrival::Sessions(s) => s.len(),
            a => panic!("{name}: arrival {a:?}"),
        };
        let rep = run_ir(&ir, None).unwrap();
        let mut got = vec![(0.0, 0.0); n];
        for &(t, s, _) in &rep.observe("first").unwrap().records {
            got[s as usize].0 = t;
        }
        for &(t, s, _) in &rep.observe("done").unwrap().records {
            got[s as usize].1 = t;
        }
        let want: Vec<(f64, f64)> = (0..n)
            .map(|i| {
                let k = i.to_string();
                (
                    ans["first"][&k].as_f64().unwrap(),
                    ans["done"][&k].as_f64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            got,
            want,
            "{name}: (first, done) per request\n{}",
            rep.text()
        );
        assert_eq!(
            rep.pool("kv").unwrap().preemptions,
            ans["preemptions"].as_u64().unwrap(),
            "{name}: preemptions"
        );
    }
}
