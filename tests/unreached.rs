//! A station no request reaches: a queue's engine no entry call runs, a
//! `nic` no relation copies over, a `stage` nothing runs. Such a program
//! used to link, and the deployment view left the station out in silence.

mod common;

use serq::{Program, ir::CStmt};

/// The workload of every program here: requests of 100 tokens.
const WORKLOAD: &str = "workload { arrive poisson(1); turn { set n = 100; } send (n); }";

fn check(src: &str) -> Result<Program, String> {
    common::compile(&common::main_source(src), WORKLOAD, &common::horizon(10.0))
}

/// An aggregated queue `name`, its entry running its engine, with `nic` in it.
fn queue(name: &str, nic: &str) -> String {
    format!(
        "queue {name} : agg {{
          device gpu {{ kv cap 1000; }}
          engine on gpu {{ tokens cap 64; scheduler {{ advance running; admit waiting; }} worker {{ forward (1); }} }}
          pool kv on gpu {{ block 16; }}
          {nic}
          decode (p) {{ {name} (p, 1); }}
        }}
        "
    )
}

/// `E1`, which the server calls, and `decls`.
fn program(decls: &str) -> String {
    format!(
        "{}{decls}
        server (n) {{ E1.decode (n); }}
        ",
        queue("E1", "")
    )
}

#[test]
fn a_stage_no_statement_runs_does_not_link() {
    check(&program("")).expect("every station reached");
    let e = check(&program(&queue("E2", ""))).expect_err("E2 is reached by nothing");
    assert!(e.contains("engine `E2` is run by no statement"), "{e}");
    assert!(e.contains("no request reaches it"), "{e}");
    assert!(
        e.contains("queue E2 : agg {"),
        "points at the declaration: {e}"
    );
    let e = check(&program("stage lonely : delay;")).expect_err("lonely is run by nothing");
    assert!(e.contains("stage `lonely` is run by no statement"), "{e}");
    assert!(
        e.contains("stage lonely : delay;"),
        "points at the declaration: {e}"
    );
}

#[test]
fn a_nic_no_copy_crosses_does_not_link() {
    let e = check(&program("").replacen("decode (p) {", "nic ps(1e5); decode (p) {", 1))
        .expect_err("E1's NIC carries nothing");
    assert!(
        e.contains("error[E-nic-unused]: the NIC `E1.nic` carries no copy"),
        "{e}"
    );
    assert!(
        e.contains("kvt.nixl_pull"),
        "names the relation to write: {e}"
    );
    // at the `nic` (the `decode` line of `queue` above, its 11th column),
    // not at the queue's name `E1` (1:7) (#645 A1)
    assert!(
        e.contains(" 6:11: error[E-nic-unused]"),
        "points at the NIC: {e}"
    );
    // no fix-it: an expression may read `E1.nic` (`queue(E1.nic)`), which
    // removing it would leave unknown
    assert!(
        e.contains("help: remove the `nic` of `E1` (and what reads `E1.nic`)"),
        "{e}"
    );
    assert!(!e.contains("fix-it"), "{e}");
}

/// The IR's own check, for IR read from JSON or built by a tool: a stage
/// added beside the one the program runs.
#[test]
fn ir_with_a_stage_no_statement_runs_is_refused() {
    let mut p = check(&program("stage tool : delay;").replace(
        "server (n) { E1.decode (n); }",
        "server (n) { E1.decode (n); run tool (cost(tool, 1)); }",
    ))
    .unwrap();
    let tool = p.stages.iter().position(|s| s.name == "tool").unwrap();
    assert!(
        p.blocks
            .iter()
            .flatten()
            .any(|s| matches!(s, CStmt::Run { stage, .. } if stage.base == tool))
    );
    let mut spare = p.stages[tool].clone();
    spare.name = "spare".into();
    p.stages.push(spare);
    let e = Program::from_json(&p.to_json()).unwrap_err();
    assert!(e.contains("stage `spare` is run by no statement"), "{e}");
}
