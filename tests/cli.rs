//! The binary's exit status. A run whose report counts `stuck` sessions is a
//! result to read with care, not a failure: `stuck` counts a no-progress
//! re-preemption once per session and rises under ordinary overload too
//! (#64 holds the livelock criterion that would change this).

use std::path::Path;
use std::process::Command;

fn seq_lang() -> Command {
    Command::new(env!("CARGO_BIN_EXE_serq"))
}

/// The program of `tests/pool_semantics.rs::a_hold_that_can_never_fit_is_reported_stuck`:
/// a hold that fits at admission and can never grow to what its body needs.
const STUCK: &str = r#"
    pool reqs { cap 4; admit via engine; }
    pool kv { cap 160; block 16; evict lru; preempt lifo; }
    stage engine : step { budget 1000; chunk 0; cost 1; memory kv; }
    workload { arrive batch(1); }
    session {
      hold reqs (1), kv (100) reserve (100) {
        run engine prefill (100) growing kv;
        run engine decode (100) growing kv;
      }
      end;
    }
    run { horizon 400; }
"#;

#[test]
fn a_stuck_run_prints_its_report_and_exits_0() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-stuck");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("stuck.sq");
    std::fs::write(&file, STUCK).unwrap();
    let out = seq_lang().arg("run").arg(&file).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("stuck: 1 session(s)"), "{text}");
    let out = seq_lang()
        .arg("run")
        .arg(&file)
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
        "stage svc : delay;\nsession { run nowhere (1); end; }\nrun { horizon 1; }\n",
    )
    .unwrap();
    let out = seq_lang().arg("run").arg(&file).output().unwrap();
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
        "stage svc : delay; workload { arrive batch(1); init { set c = 5; set K = 10; } }\n\
         session { branch (c / K) { run svc (1); } end; } run { horizon 10; }\n",
    )
    .unwrap();
    let out = seq_lang().arg("run").arg(&file).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("guard.sq: `branch (c / K)`: the guard is 0.5, not 0 or 1"),
        "{stderr}"
    );
    assert!(!stderr.contains("panicked"), "{stderr}");
}
