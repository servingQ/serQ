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

/// The iterations of a logged run, one line each in the form the removed
/// `SERQ_TRACE_ITER` switch printed (#728), so the schedules the tests were
/// written in stay as written: `ITER t s:turn:mn` per request of the batch,
/// `m` its mode (`p` prefill, `d` decode) and `n` its tokens (the `iter`
/// event's `each`). The turn is the session's last `turn` (a leg takes its
/// session's when it is forked), the mode its request's last `status`.
pub fn iter_lines(log: &serq::engine::log::Log) -> Vec<String> {
    use serq::engine::log::{EventKind, Who};
    use serq::frontend::ast::RunMode;
    use std::collections::HashMap;
    let key = |w: &Who| (w.serial, w.leg);
    let mut turn: HashMap<(u64, Option<u32>), u32> = HashMap::new();
    let mut mode: HashMap<(u64, Option<u32>), RunMode> = HashMap::new();
    let mut lines = vec![];
    for e in &log.events {
        match &e.kind {
            EventKind::Turn { who, turn: n } => {
                turn.insert(key(who), *n);
            }
            EventKind::Fork { who } => {
                let n = turn.get(&(who.serial, None)).copied().unwrap_or(0);
                turn.insert(key(who), n);
            }
            EventKind::Status { who, mode: m, .. } => {
                mode.insert(key(who), *m);
            }
            EventKind::Iter { reqs, each, .. } => {
                let parts: Vec<String> = reqs
                    .iter()
                    .zip(each)
                    .map(|(w, n)| {
                        let m = match mode.get(&key(w)) {
                            Some(RunMode::Decode) => "d",
                            _ => "p",
                        };
                        let tn = turn.get(&key(w)).copied().unwrap_or(0);
                        format!("{}:{tn}:{m}{n}", w.serial)
                    })
                    .collect();
                lines.push(format!("ITER {:.4} {}", e.t, parts.join(" ")));
            }
            _ => {}
        }
    }
    lines
}
