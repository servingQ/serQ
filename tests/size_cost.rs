//! A request fixes its demand; the deployment fixes what serving it costs.
//! These tests exercise the public compiler and JSON validator directly.
mod common;

use serq::{Program, compile_source, run_ir};

fn compile(server: &str, turn: &str) -> Result<Program, String> {
    compile_source(
        &format!(
            r#"fn main() {{
        pool mem {{ cap 10; }}
        pool other {{ cap 10; }}
        stage svc : fifo;
        stage tool : delay;
        workload {{ arrive batch(1); turn {{ {turn} }}
            session {{ turn; end; }} }}
        server {{ {server} }}
    }}"#
        ),
        &common::horizon(20.0),
    )
}

#[test]
fn same_request_two_server_models() {
    // One request of three items; the two deployments use one or two seconds
    // per item. Allocation is independent of the compute rate.
    for (seconds, expected) in [(1, 3.0), (2, 6.0)] {
        let p = compile(&format!("hold mem (cost(mem, items)) {{ run svc (cost(svc, {seconds} * items)); }} observe finished = now;"), "set items = 3;").unwrap();
        let q = Program::from_json(&p.to_json()).unwrap();
        assert_eq!(p.to_json(), q.to_json());
        let report = run_ir(&q, None).unwrap();
        let json: serde_json::Value = serde_json::from_str(&report.json()).unwrap();
        assert_eq!(json["observes"]["finished"]["mean"], expected);
    }
}

#[test]
fn server_cannot_rewrite_request_size() {
    let e = compile(
        "set items = 1; run svc (cost(svc, items));",
        "set items = 3;",
    )
    .unwrap_err();
    assert!(
        e.contains("server cannot assign request size `items`"),
        "{e}"
    );
}

#[test]
fn resource_use_requires_explicit_conversion() {
    for body in [
        "run svc (items);",
        "hold mem (items) { run svc (cost(svc, 1)); }",
    ] {
        let e = compile(body, "set items = 3;").unwrap_err();
        assert!(e.contains("requires its cost type"), "{e}");
    }
}

#[test]
fn costs_cannot_cross_resources_or_be_recast() {
    for body in [
        "hold mem (cost(other, items)) { run svc (cost(svc, 1)); }",
        "run svc (cost(tool, items));",
        "set c = cost(other, items); hold mem (c) { run svc (cost(svc, 1)); }",
        "run svc (cost(svc, cost(tool, items)));",
    ] {
        assert!(compile(body, "set items = 3;").is_err(), "{body}");
    }
}

#[test]
fn workload_cannot_supply_server_cost() {
    for turn in [
        "set items = cost(svc, 3);",
        "set c = cost(svc, 3); set items = c;",
    ] {
        assert!(compile("run svc (cost(svc, 1));", turn).is_err());
    }
}

#[test]
fn cost_aliases_preserve_the_resource_type() {
    compile(
        "set c = cost(svc, items); set alias = c * 2; run svc (alias);",
        "set items = 3;",
    )
    .unwrap();
    let e = compile(
        "set c = cost(svc, items); set alias = c * 2; hold mem (alias) { run svc (c); }",
        "set items = 3;",
    )
    .unwrap_err();
    assert!(e.contains("requires its cost type"), "{e}");
}

#[test]
fn json_cannot_omit_the_contract_or_erase_a_conversion() {
    let p = compile("run svc (cost(svc, items));", "set items = 3;").unwrap();
    let mut json: serde_json::Value = serde_json::from_str(&p.to_json()).unwrap();
    json.as_object_mut().unwrap().remove("sides");
    assert!(Program::from_json(&json.to_string()).is_err());
    let mut q = p.clone();
    q.attr_types.clear();
    assert!(q.validate().is_err());
    let mut q = p.clone();
    q.sides.clear();
    assert!(q.validate().is_err());
    let mut q = p;
    for st in q.blocks.iter_mut().flatten() {
        if let serq::ir::CStmt::Run { work, .. } = st {
            *work = serq::ir::CExpr::Num(3.0);
        }
    }
    assert!(q.validate().unwrap_err().contains("requires its cost type"));
}

#[test]
fn client_tool_cost_does_not_grant_server_resource_access() {
    let source = r#"fn main() {
        stage svc : fifo; stage tool : delay;
        workload { arrive batch(1); turn { set items = 3; }
          session { turn; run tool (cost(tool, 2)); end; } }
        server { run svc (cost(svc, items)); }
    }"#;
    compile_source(source, &common::horizon(10.0)).unwrap();
    let bad = source.replace("run tool (cost(tool, 2))", "run svc (cost(svc, 2))");
    assert!(
        compile_source(&bad, &common::horizon(10.0))
            .unwrap_err()
            .contains("workload cannot")
    );
}

#[test]
fn cache_cost_is_ready_even_when_a_hold_exits_early() {
    let e = compile(
        "hold mem (cost(mem, 1)) { release mem; set c = cost(mem, 1); } cache(c);",
        "",
    )
    .unwrap_err();
    assert!(e.contains("read before every path"), "{e}");
    compile(
        "set c = cost(mem, 1); hold mem (cost(mem, 1)) { release mem; } cache(c);",
        "",
    )
    .unwrap();
}

#[test]
fn lease_seconds_are_not_pool_units() {
    let e = compile(
        "hold mem (cost(mem, 1)) { run svc (cost(svc, 1)); } lease mem (cost(mem, 7));",
        "",
    )
    .unwrap_err();
    assert!(e.contains("lease duration"), "{e}");
}

#[test]
fn conversions_cannot_hide_inside_workload_predicates() {
    for turn in [
        "set items = cost(tool, 1) == cost(tool, 1);",
        "observe x = cost(tool, 1);",
    ] {
        let e = compile("run svc (cost(svc, 1));", turn).unwrap_err();
        assert!(e.contains("sizes, not costs"), "{e}");
    }
}

#[test]
fn declarations_cannot_consume_session_costs() {
    let p = compile("run svc (cost(svc, 1));", "").unwrap();
    let mut q = p.clone();
    q.stages[0].kind = serq::ir::CStageKind::Ps(serq::ir::CExpr::Cost(
        serq::ir::CostTarget::Pool { base: 0, count: 1 },
        Box::new(serq::ir::CExpr::Num(7.0)),
    ));
    assert!(q.validate().unwrap_err().contains("ordinary quantities"));
}

#[test]
fn json_cannot_retype_builtins_or_restore_workload_authority_inside_server() {
    use serq::ir::{CStmt, Side, ValueType};
    let p = compile("hold mem (cost(mem, 1)) { run svc (cost(svc, 1)); }", "").unwrap();
    let mut q = p.clone();
    q.attr_types[q.slot_new] = ValueType::Value;
    assert!(q.validate().unwrap_err().contains("built-in"));
    let mut q = p;
    let body = q
        .blocks
        .iter()
        .flatten()
        .find_map(|s| match s {
            CStmt::Hold { body, .. } => Some(*body),
            _ => None,
        })
        .unwrap();
    q.sides[body][0] = Side::Workload;
    assert!(q.validate().unwrap_err().contains("server body"));
}

#[test]
fn serving_vocabulary_converts_quantities_not_already_converted_costs() {
    for body in [
        "prefill(cost(svc, 3));",
        "set c = cost(svc, 3); prefill(c);",
    ] {
        let src = format!(
            "fn main() {{ stage svc : delay; workload {{ arrive batch(1); session {{ turn; end; }} }} server {{ {body} }} }}"
        );
        // Explicit on: this stage's name need not be the vocabulary's default.
        let src = src.replace("prefill(", "prefill on svc (");
        assert!(
            compile_source(&src, &common::horizon(10.0))
                .unwrap_err()
                .contains("ordinary quantity")
        );
    }
}

#[test]
fn family_annotations_check_written_indices_before_projecting_the_type() {
    for index in ["missing", "99", "~uniform(0, 2)"] {
        let src = format!(
            "fn main() {{ stage svc[2] : delay; workload {{ arrive batch(1); session {{ turn; end; }} }} server {{ run svc[0] (cost(svc[{index}], 1)); }} }}"
        );
        assert!(
            compile_source(&src, &common::horizon(10.0)).is_err(),
            "{src}"
        );
    }
}

#[test]
fn workload_cannot_supply_a_cost_for_server_growth_or_load() {
    for action in ["grow mem(c);", "load mem(c);"] {
        let src = format!(
            "fn main() {{ pool mem {{ cap 10; }} workload {{ arrive batch(1); session {{ set c = cost(mem, 3); hold mem(cost(mem, 1)) {{ turn; }} end; }} }} server {{ {action} }} }}"
        );
        let e = compile_source(&src, &common::horizon(10.0)).unwrap_err();
        assert!(e.contains("workload cannot"), "{e}");
    }
}

#[test]
fn while_preserves_cost_initialization_and_statement_authority() {
    for body in [
        "while (c > cost(svc, 0)) { set c = cost(svc, 1); run svc(c); }",
        "while (items > 0) { set c = cost(svc, 1); run svc(c); } run svc(c);",
    ] {
        let e = compile(body, "set items = 0;").unwrap_err();
        assert!(e.contains("read before every path"), "{e}");
    }
    let mut p = compile(
        "while (items > 0) { run svc(cost(svc, 1)); }",
        "set items = 0;",
    )
    .unwrap();
    let body = p
        .blocks
        .iter()
        .flatten()
        .find_map(|s| match s {
            serq::ir::CStmt::While(_, body) => Some(*body),
            _ => None,
        })
        .unwrap();
    p.sides[body][0] = serq::ir::Side::Workload;
    assert!(p.validate().unwrap_err().contains("server body"));
}
