#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A deployment for the CLI tests; its workload is `WORKLOAD`, written as a
/// file of its own and given with `--workload`.
pub const PROGRAM: &str = "use \"std/args\"; let rate = args.number(\"rate\", 1);\nstage svc : fifo;\nserver { run svc (cost(svc, rate)); }\n\n";

/// `PROGRAM`'s workload.
pub const WORKLOAD: &str = "workload { arrive batch(1); session { turn; end; } }\n";

pub struct Fixture(pub PathBuf);

impl Fixture {
    pub fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "serq-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }

    pub fn write(&self, file: &str, contents: &str) {
        let path = self.0.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    pub fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_serq"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn failure(output: &Output, code: i32, fragments: &[&str]) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(code), "{stderr}");
    assert!(output.stdout.is_empty(), "{:?}", output.stdout);
    for fragment in fragments {
        assert!(stderr.contains(fragment), "missing {fragment:?}: {stderr}");
    }
}

/// Existing semantic fixtures describe a main body; complete example files
/// already contain their entry point. The entrypoint tests use the public API
/// directly, so this builder cannot make an implicit program pass those checks.
pub fn main_source(body: &str) -> String {
    if body.contains("fn main()") {
        body.to_string()
    } else {
        format!("fn main() {{ {body}\n}}")
    }
}

/// Execution conditions belong to the test, separately from its model text.
pub fn horizon(horizon: f64) -> serq::Overrides {
    serq::Overrides {
        horizon: Some(horizon),
        ..Default::default()
    }
}

/// A number of a vLLM oracle scenario, 0 when it is absent.
fn num(v: &serde_json::Value, k: &str) -> f64 {
    v.get(k).and_then(serde_json::Value::as_f64).unwrap_or(0.0)
}

/// The engine of a vLLM oracle scenario (`tools/oracle/*.json`) as the
/// arguments of `examples/oracle/vllm_request.sq`. This and
/// `oracle_sessions` are the one mapping from a scenario to the program,
/// which `tests/vllm_oracle.rs` and `tests/vllm_target_oracle.rs` both read.
pub fn oracle_engine(sc: &serde_json::Value) -> serq::Overrides {
    let mut ov = horizon(100000.0);
    for (k, v) in [
        ("bs", num(sc, "block_size")),
        ("B", num(sc, "budget")),
        ("blocks", num(sc, "num_blocks") - 1.0), // the null block
        ("max_seqs", num(sc, "max_seqs")),
    ] {
        ov.set_num(k, v).unwrap();
    }
    // vLLM's 0 is no cap, which the program's default `inf` already is
    if num(sc, "chunk") > 0.0 {
        ov.set_num("chunk", num(sc, "chunk")).unwrap();
    }
    // long_prefill_token_threshold_adaptive (config/scheduler.py:87-91)
    if sc.get("adaptive").and_then(serde_json::Value::as_bool) == Some(true) {
        ov.set_num("adaptive", 1.0).unwrap();
    }
    ov
}

/// The requests of a vLLM oracle scenario as the program's explicit
/// sessions: the prompt, the output length `o` and the arrival step.
pub fn oracle_sessions(sc: &serde_json::Value) -> Vec<Vec<(&'static str, f64)>> {
    sc["requests"]
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
        .collect()
}

/// The vLLM oracle scenarios in `dir`, sorted: each `<name>.json` with the
/// oracle's answer beside it, `<name>.out.json`.
pub fn oracle_scenarios(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| {
            let f = e.unwrap().file_name().into_string().unwrap();
            f.strip_suffix(".out.json").map(str::to_string)
        })
        .filter(|n| dir.join(format!("{n}.json")).exists())
        .collect();
    names.sort();
    names
}

/// What a pair's messages name its files: a deployment error begins
/// `deployment.sq: L:C:`, a workload error `workload.sq: L:C:`.
pub const NAMES: (&str, &str) = ("deployment.sq", "workload.sq");

/// Compile a deployment with the workload file `workload` beside it, as
/// `serq run deployment.sq --workload workload.sq` does.
pub fn compile(
    deployment: &str,
    workload: &str,
    ov: &serq::Overrides,
) -> Result<serq::Program, String> {
    serq::compile_paired(deployment, workload, NAMES, ov)
}

/// `compile`, for inspection: text without a horizon is given one.
pub fn inspected(
    deployment: &str,
    workload: &str,
    ov: &serq::Overrides,
) -> Result<serq::Program, String> {
    serq::compile_paired_inspected(deployment, workload, NAMES, ov)
}

/// Run a pair (`compile`).
pub fn run(deployment: &str, workload: &str, ov: &serq::Overrides) -> Result<serq::Report, String> {
    serq::run_paired(deployment, workload, NAMES, ov, None)
}

/// An example (or tutorial) program and the workload `examples/pairs.json`
/// pairs it with, compiled; `path` relative to the crate.
pub fn example(path: &str, ov: &serq::Overrides) -> Result<serq::Program, String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let program = root.join(path);
    let workload = serq::paired_workload(&program).unwrap_or_else(|| panic!("{path}: no pair"));
    serq::load_paired(&program, &workload, ov)
}

/// `example`, run; a trace is read beside the workload's file.
pub fn run_example(path: &str, ov: &serq::Overrides) -> Result<serq::Report, String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workload = serq::paired_workload(&root.join(path)).expect("a pair");
    serq::run_ir(&example(path, ov)?, workload.parent())
}
