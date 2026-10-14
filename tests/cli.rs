//! The binary's exit status. A run whose report counts `stuck` sessions is a
//! result to read with care, not a failure: `stuck` counts a no-progress
//! re-preemption once per session and rises under ordinary overload too
//! (#64 holds the livelock criterion that would change this).

mod common;

use std::path::Path;
use std::process::Command;

fn serq() -> Command {
    Command::new(env!("CARGO_BIN_EXE_serq"))
}

/// The program of `tests/pool_semantics.rs::a_hold_that_can_never_fit_is_reported_stuck`:
/// a hold that fits at admission and can never grow to what its body needs.
const STUCK: &str = r#"
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

/// `STUCK`'s workload.
const STUCK_WORKLOAD: &str = r#"
        workload { arrive batch(1);
          session { turn;
            end;

          }
        }
"#;

#[test]
fn a_stuck_run_prints_its_report_and_exits_0() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-stuck");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("stuck.sq");
    std::fs::write(&file, common::main_source(STUCK)).unwrap();
    let workload = dir.join("w.sq");
    std::fs::write(&workload, STUCK_WORKLOAD).unwrap();
    let out = serq()
        .arg("run")
        .arg(&file)
        .arg("--workload")
        .arg(&workload)
        .args(["--horizon", "400"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("stuck: 1 session(s)"), "{text}");
    let out = serq()
        .arg("run")
        .arg(&file)
        .arg("--workload")
        .arg(&workload)
        .args(["--horizon", "400"])
        .arg("--json")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("\"stuck\":1"), "{text}");
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
