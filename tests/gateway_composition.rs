//! Executable design witnesses for the workload/system boundary and a
//! gateway's admission scope. Expected times follow from the constant
//! services in the example, not a production scheduler approximation.
use seq::{Overrides, parser, run_source};

const PROGRAM: &str = concat!(
    include_str!("../docs/design/programs/gateway-implementation.seq"),
    include_str!("../docs/design/programs/gateway-model.seq"),
    include_str!("../docs/design/programs/gateway-workload.seq"),
    include_str!("../docs/design/programs/gateway-run.seq"),
);

#[test]
fn moving_the_pd_entry_out_of_the_workload_preserves_the_entire_ir() {
    let after = include_str!("../programs/llmd_pd.seq");
    let before = after
        .replace("server { gw.route(); }", "")
        .replace("      request;", "      request gw;");
    let ir = |s| {
        seq::compile_source(s, &Overrides::default())
            .unwrap()
            .to_json()
    };
    assert_eq!(ir(&before), ir(after));
}

#[test]
fn gateway_admission_covers_prefill_copy_and_decode() {
    let r = run_source(PROGRAM, &Overrides::default(), None).unwrap();
    // Both arrive at zero. Remote: router 1 + prefill 2 + copy 2 + decode 3
    // = 8. The second cannot enter until 8; local: router 1 + decode 3 = 4.
    assert_eq!(r.observe("admitted").unwrap().samples, [0.0, 8.0]);
    assert_eq!(r.observe("client_done").unwrap().samples, [8.0, 12.0]);
    assert_eq!(r.observe("source_after_copy").unwrap().samples, [0.0]);
    assert_eq!(r.stage("gw").unwrap().mean_service, 1.0);
    assert_eq!(r.stage("P").unwrap().completed, 1);
    assert_eq!(
        r.stages_named("nic")
            .iter()
            .map(|s| s.completed)
            .sum::<u64>(),
        1
    );
    assert_eq!(r.ended, 2);
    assert!((r.pool("gw.inflight").unwrap().mean_used - 12.0 / 20.0).abs() < 1e-12);
}

#[test]
fn changing_the_system_does_not_change_the_workload() {
    let direct = PROGRAM.replace("server { gw.route(); }", "server { D[0].decode(prompt); }");
    assert_eq!(
        parser::parse(PROGRAM).unwrap().workload,
        parser::parse(&direct).unwrap().workload
    );
    let r = run_source(&direct, &Overrides::default(), None).unwrap();
    // The same two clients now enter a delay-stage decoder directly: each
    // takes three seconds. An unused gateway never runs or admits anything.
    assert_eq!(r.observe("client_done").unwrap().samples, [3.0, 3.0]);
    assert_eq!(r.pool("gw.inflight").unwrap().admissions, 0);
    assert_eq!(r.stage("gw").unwrap().completed, 0);
    assert_eq!(r.ended, 2);
}

#[test]
fn router_service_and_inflight_admission_are_different_limits() {
    let ov = Overrides {
        lets: vec![("gateway_limit".into(), parser::parse_expr("2").unwrap())],
        ..Default::default()
    };
    let r = run_source(PROGRAM, &ov, None).unwrap();
    // Both hold an inflight slot immediately, but the FIFO router handles
    // them in [0,1] and [1,2]. The local request finishes at 2+3=5; the
    // remote request at 1+2+2+3=8. The decoder is a delay stage in this toy.
    assert_eq!(r.observe("admitted").unwrap().samples, [0.0, 0.0]);
    assert_eq!(r.observe("client_done").unwrap().samples, [5.0, 8.0]);
    assert_eq!(r.stage("gw").unwrap().completed, 2);
}

#[test]
fn implementation_changes_capacity_and_service_but_not_topology() {
    let ov = Overrides {
        lets: [
            ("kv_capacity", "200"),
            ("router_work", "0.5"),
            ("prefill_work", "4"),
            ("transfer_work", "1"),
            ("decode_work", "2"),
        ]
        .into_iter()
        .map(|(n, e)| (n.into(), parser::parse_expr(e).unwrap()))
        .collect(),
        ..Default::default()
    };
    let before = seq::compile_source(PROGRAM, &Overrides::default()).unwrap();
    let after = seq::compile_source(PROGRAM, &ov).unwrap();
    assert_eq!(before.stages.len(), after.stages.len());
    assert_eq!(before.pools.len(), after.pools.len());
    let r = seq::run_ir(&after, None).unwrap();
    // Counts and edges remain fixed: two decoders, two NICs, one prefiller.
    assert_eq!(r.stages_named("D").len(), 2);
    assert_eq!(r.stages_named("nic").len(), 2);
    assert_eq!(r.stages_named("P").len(), 1);
    // Remote: 0.5+4+1+2=7.5; local: 0.5+2=2.5, after the first releases.
    assert_eq!(r.observe("admitted").unwrap().samples, [0.0, 7.5]);
    assert_eq!(r.observe("client_done").unwrap().samples, [7.5, 10.0]);
}

// This helper is a lowering witness for expression hooks, not a model/impl
// frontend. It fills only the two slots offered by a fixed step-stage model.
fn step_implementation(budget: &str, cost: &str) -> Result<seq::Program, String> {
    let mut model = parser::parse(
        "stage engine : step { cost 0; }
         workload { arrive batch(1); }
         session { run engine prefill (5); observe done = now; end; }
         run { horizon 20; }",
    )
    .unwrap();
    let seq::ast::StageKind::Step(step) = &mut model.stages[0].kind else {
        unreachable!()
    };
    step.budget = parser::parse_expr(budget).unwrap();
    step.cost = parser::parse_expr(cost).unwrap();
    let ir = seq::link::link(&model, &Overrides::default()).map_err(|e| e.to_string())?;
    ir.validate()?;
    Ok(ir)
}

#[test]
fn implementation_expressions_keep_their_declared_evaluation_moments() {
    for (budget, done) in [("2", 5.5), ("3", 4.5)] {
        let ir = step_implementation(budget, "1 + 0.5 * ntok").unwrap();
        let r = seq::run_ir(&ir, None).unwrap();
        // Five tokens: budget 2 gives batches [2,2,1], costs [2,2,1.5];
        // budget 3 gives [3,2], costs [2.5,2]. The cost is not evaluated
        // once at configuration time or hoisted outside the step.
        assert_eq!(r.observe("done").unwrap().samples, [done]);
    }
    // ntok belongs to a selected batch; using it to choose the budget for
    // that batch would read a value that does not exist at that moment.
    let err = step_implementation("ntok", "1").unwrap_err();
    assert!(err.contains("ntok"), "{err}");
}
