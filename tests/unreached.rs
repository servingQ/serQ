//! A station no request reaches: a queue's engine no entry call runs, a
//! `nic` no relation copies over, a `stage` nothing runs. Such a program
//! used to link, and the deployment view left the station out in silence.

mod common;

use serq::{Program, compile_source, ir::CStmt};

fn check(src: &str) -> Result<Program, String> {
    compile_source(&common::main_source(src), &common::horizon(10.0))
}

/// A decode queue `name`, its entry running its engine, with `nic` in it.
fn queue(name: &str, nic: &str) -> String {
    format!(
        "queue {name} : decode {{
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
        workload {{ arrive poisson(1); turn {{ set n = 100; }} }}
        server {{ E1.decode (n); }}
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
        e.contains("queue E2 : decode {"),
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
    assert!(e.contains("the NIC `E1.nic` carries no copy"), "{e}");
    assert!(
        e.contains("kvt.nixl_pull"),
        "names the relation to write: {e}"
    );
}

/// The IR's own check, for IR read from JSON or built by a tool: a stage
/// added beside the one the program runs.
#[test]
fn ir_with_a_stage_no_statement_runs_is_refused() {
    let mut p = check(&program("stage tool : delay;").replace(
        "server { E1.decode (n); }",
        "server { E1.decode (n); run tool (cost(tool, 1)); }",
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
