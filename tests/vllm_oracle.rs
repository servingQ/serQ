//! Differential test against the real vLLM v1 scheduler (upstream
//! `ref/vllm` at 0c87a197, `tools/vllm_oracle.py` driven with a fake model
//! runner, rbln platform plugin disabled with `VLLM_PLUGINS=`).
//!
//! One program, `examples/oracle/vllm_request.sq`, is compiled to IR once per
//! scenario of `tools/oracle/*.json` (the scenario's engine as constants),
//! and the scenario's requests become the IR's explicit sessions. The
//! result is committed as `tools/oracle/<name>.ir.json`: the IR is the
//! artifact both sides use. This test runs it (an iteration costs 1) and
//! compares with the oracle's answer (`*.out.json`: the step of every
//! request's first and last token, the number of preemptions); the Lean
//! theorems of serving-queue-theory are generated from the same files.
//! `oracle_ir_files_are_current` fails if a file is stale; regenerate with
//! `SERQ_BLESS=1 cargo test --release --test vllm_oracle` (`make oracle-ir`).
//!
//! The scenarios: `preempt` (the second request cannot grow its next chunk
//! and preempts itself, vLLM `running[-1]`), `chunked` (a 3000-token prompt
//! in 1024-token chunks, later requests share what its last chunk leaves),
//! `seqcap` (`max_num_seqs = 2` admits two of four), `hol` (FCFS with
//! head-of-line blocking on memory), `mixed` (mixed lengths and arrivals
//! on a small pool), `longchunk` (`long_prefill_token_threshold`), `alone`
//! (the cap is lifted while one request is eligible, scheduler.py:606-616:
//! the first request takes the whole budget; CPU oracle only, no A100 run).

use std::path::Path;

use serde_json::Value;
use serq::frontend::parser;
use serq::{Overrides, compile_source_at, program_path, run_ir};

fn dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/oracle")
}

fn num(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

fn read(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir().join(name)).unwrap()).unwrap()
}

/// The IR of a scenario: the program compiled with the scenario's engine,
/// the requests as explicit sessions.
fn oracle_ir(name: &str) -> serq::Program {
    let sc: Value =
        serde_json::from_str(&std::fs::read_to_string(dir().join(format!("{name}.json"))).unwrap())
            .unwrap();
    let set = |k: &str, v: f64| (k.to_string(), parser::parse_expr(&format!("{v}")).unwrap());
    let ov = Overrides {
        lets: vec![
            set("bs", num(&sc, "block_size")),
            set("B", num(&sc, "budget")),
            set("blocks", num(&sc, "num_blocks") - 1.0), // the null block
            set("max_seqs", num(&sc, "max_seqs")),
            set("chunk", num(&sc, "chunk")),
        ],
        ..Default::default()
    };
    let path = program_path("vllm_request");
    let src = std::fs::read_to_string(&path).unwrap();
    let reqs = sc["requests"].as_array().unwrap();
    let sessions: Vec<Vec<(&str, f64)>> = reqs
        .iter()
        .map(|q| {
            vec![
                ("prompt", num(q, "prompt")),
                ("o", num(q, "out")),
                ("arrive", num(q, "arrive")),
            ]
        })
        .collect();
    // its `use` reads the library next to the program
    compile_source_at(&src, path.parent(), &ov)
        .unwrap()
        .with_sessions(&sessions)
        .unwrap()
}

fn check(name: &str) {
    let ans = read(&format!("{name}.out.json"));
    let ir = serq::Program::from_json(
        &std::fs::read_to_string(dir().join(format!("{name}.ir.json"))).unwrap(),
    )
    .unwrap();
    let n = match &ir.arrival {
        serq::ir::CArrival::Sessions(s) => s.len(),
        a => panic!("{name}: arrival {a:?}"),
    };
    let rep = run_ir(&ir, None).unwrap();
    // every observation records the serial of the session that made it
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

fn scenarios() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir())
        .unwrap()
        .filter_map(|e| {
            let f = e.unwrap().file_name().into_string().unwrap();
            f.strip_suffix(".out.json").map(str::to_string)
        })
        .filter(|n| dir().join(format!("{n}.json")).exists())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "alone",
            "chunked",
            "hol",
            "longchunk",
            "mixed",
            "preempt",
            "seqcap"
        ],
        "the oracle scenarios"
    );
    names
}

#[test]
fn every_oracle_scenario() {
    for n in scenarios() {
        check(&n);
    }
}

#[test]
fn oracle_ir_files_are_current() {
    let bless = std::env::var_os("SERQ_BLESS").is_some();
    for n in scenarios() {
        let path = dir().join(format!("{n}.ir.json"));
        let want = oracle_ir(&n).to_json() + "\n";
        if bless {
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
}
