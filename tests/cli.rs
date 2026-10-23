//! The binary's exit status: 0, done; 1, no report (the program does not
//! load, or the run fails); 2, a usage error; 3, the report is printed and
//! the run ended blocked, with a session that never completes
//! (`Report::blocked`, `docs/design/blocked-run.md`). A run whose report
//! counts `stuck` sessions and nothing blocked exits 0: `stuck` counts a
//! no-progress re-preemption, which a thrashing run that completes counts
//! too (#480).

mod common;

use std::path::Path;
use std::process::Command;

fn serq() -> Command {
    Command::new(env!("CARGO_BIN_EXE_serq"))
}

/// Run `src` under `workload` to `horizon`, in text and in JSON: (exit
/// status, text report, stderr, JSON report).
fn run_both(
    name: &str,
    src: &str,
    workload: &str,
    horizon: &str,
) -> (Option<i32>, String, String, String) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("deployment.sq");
    std::fs::write(&file, common::main_source(src)).unwrap();
    let w = dir.join("w.sq");
    std::fs::write(&w, workload).unwrap();
    let run = |json: bool| {
        let mut c = serq();
        c.arg("run")
            .arg(&file)
            .arg("--workload")
            .arg(&w)
            .args(["--horizon", horizon]);
        if json {
            c.arg("--json");
        }
        c.output().unwrap()
    };
    let text = run(false);
    let json = run(true);
    assert_eq!(text.status.code(), json.status.code());
    (
        text.status.code(),
        String::from_utf8_lossy(&text.stdout).into(),
        String::from_utf8_lossy(&text.stderr).into(),
        String::from_utf8_lossy(&json.stdout).into(),
    )
}

/// The program of `tests/pool_semantics.rs::a_hold_that_can_never_fit_is_reported_outgrown`:
/// a hold that fits at admission and can never grow to what its body needs.
const LIVELOCK: &str = r#"
        device gpu { kv cap 160; }
        engine llm on gpu {
          reqs cap 4;
          tokens cap 1000;
          scheduler {
            advance running;
            admit waiting while (running.preempted == 0) taking reqs (1), kv (100) reserve (100);
          }
          worker { forward (1); }
        }
        pool reqs on llm { }
        pool kv on gpu { block 16; evict lru; preempt lifo; }
        server { llm (100, 101); }

"#;

/// `LIVELOCK`'s workload.
const ONE: &str = r#"
        workload { arrive batch(1);
          session { turn;
            end;

          }
        }
"#;

/// The livelock: its request grows to 161 tokens on a 160-token pool and
/// preempts itself for ever (`outgrown`). The report is printed whole,
/// then the run fails with 3, which a script under `set -e` stops at and a
/// reader tells from a program that does not load (1).
#[test]
fn a_livelocked_run_prints_its_report_and_exits_3() {
    let (code, text, err, json) = run_both("cli-livelock", LIVELOCK, ONE, "400");
    assert_eq!(code, Some(3), "{text}\n{err}");
    assert!(text.contains("stuck: 1 session(s)"), "{text}");
    assert!(text.contains("outgrown: 1 session(s)"), "{text}");
    assert!(err.contains("the run ended blocked"), "{err}");
    assert!(err.contains("pool `kv`"), "{err}");
    assert!(json.contains("\"outgrown\":1"), "{json}");
    assert!(json.contains("\"blocked\":true"), "{json}");
}

/// Three requests of 60 + 60 tokens on one 160-token pool, each admitted
/// with what the step's budget (32 tokens) takes of its prompt and growing
/// as it computes: together they outgrow the pool and preempt one another,
/// some of them again below the position of their previous preemption
/// (`stuck`). But each needs 120 tokens (8 blocks of 10) at most, so under
/// `preempt lifo` the oldest always goes on and all three end
/// (`docs/design/ir-v4.md` §7). Thrash, not a livelock: exit 0.
#[test]
fn a_thrashing_run_exits_0() {
    let src = r#"
        device gpu { kv cap 160; }
        engine llm on gpu {
          reqs cap 4;
          tokens cap 32;
          scheduler {
            advance running;
            admit waiting while (running.preempted == 0)
              taking reqs (1), kv (min(request.known, budget_left(llm))) reserve (16);
          }
          worker { forward (1); }
        }
        pool reqs on llm { }
        pool kv on gpu { block 16; evict lru; preempt lifo; }
        server { llm (60, 60); }
"#;
    let workload = "workload { arrive batch(3); session { turn; end; } }";
    let (code, text, err, json) = run_both("cli-thrash", src, workload, "2000");
    assert_eq!(code, Some(0), "{text}\n{err}");
    assert!(text.contains("\nstuck: "), "{text}");
    assert!(json.contains("\"ended\":3"), "{json}");
    assert!(json.contains("\"blocked\":false"), "{json}");
    assert!(err.is_empty(), "{err}");
}

#[test]
fn a_program_that_does_not_load_exits_1() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-bad");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("bad.sq");
    std::fs::write(
        &file,
        common::main_source("stage svc : delay;\nserver { run nowhere (cost(nowhere, 1));\n}\n\n"),
    )
    .unwrap();
    let workload = dir.join("w.sq");
    std::fs::write(&workload, "workload { session { turn; end; \n} }\n").unwrap();
    let out = serq()
        .arg("run")
        .arg(&file)
        .arg("--workload")
        .arg(&workload)
        .args(["--horizon", "400"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("nowhere"));
}

#[test]
fn a_runtime_guard_error_exits_1_without_a_panic() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-runtime-guard");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("guard.sq");
    std::fs::write(
        &file,
        common::main_source(
            "stage svc : delay;
        server (c, K) { if (c / K) { run svc (cost(svc, 1)); }
        }
",
        ),
    )
    .unwrap();
    let workload = dir.join("w.sq");
    std::fs::write(
        &workload,
        "workload { arrive batch(1); init { set c = 5; set K = 10; } send (c, K);
          session { turn; end;
          }
        }
",
    )
    .unwrap();
    let out = serq()
        .arg("run")
        .arg(&file)
        .arg("--workload")
        .arg(&workload)
        .args(["--horizon", "400"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("guard.sq: `if (c / K)`: the guard is 0.5, not 0 or 1"),
        "{stderr}"
    );
    assert!(!stderr.contains("panicked"), "{stderr}");
}
