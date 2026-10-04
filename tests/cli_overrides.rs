mod common;
use common::{Fixture, PROGRAM, failure};
use serq::frontend::parser;
use serq::{Overrides, compile_source};

#[test]
fn misspelled_and_empty_override_names_do_not_produce_ir() {
    let f = Fixture::new();
    f.write("model.sq", PROGRAM);
    for name in ["raet", "Rate", ""] {
        failure(
            &f.run(&["ir", "model.sq", "--set", &format!("{name}=2")]),
            if name.is_empty() { 2 } else { 1 },
            &["--set"],
        );
    }
    failure(
        &f.run(&["ir", "model.sq", "--set", "raet=2"]),
        1,
        &["unknown --set constant `raet`", "available constants: rate"],
    );
}

#[test]
fn overrides_keep_last_value_and_declaration_order() {
    let src = "let rate = 1; let doubled = rate * 2; workload { arrive poisson(doubled); } run { horizon 10; }";
    let ov = Overrides {
        lets: vec![
            ("rate".into(), parser::parse_expr("2").unwrap()),
            ("rate".into(), parser::parse_expr("3").unwrap()),
        ],
        ..Default::default()
    };
    let p = compile_source(src, &ov).unwrap();
    assert_eq!(p.arrival, serq::ir::CArrival::Poisson(6.0));
    let f = Fixture::new();
    f.write("model.sq", src);
    let out = f.run(&["ir", "model.sq", "--set", "rate=2", "--set", "rate=3"]);
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["arrival"]["Poisson"], 6.0);
}

#[test]
fn sdk_cannot_inject_an_undeclared_constant() {
    let ov = Overrides {
        lets: vec![("outside".into(), parser::parse_expr("1").unwrap())],
        ..Default::default()
    };
    let err = compile_source("run { horizon outside; }", &ov).unwrap_err();
    assert!(err.contains("unknown --set constant `outside`"));
    assert!(err.contains("available constants: (none)"));
}

#[test]
fn arrival_override_applies_to_run_and_ir() {
    let f = Fixture::new();
    f.write(
        "model.sq",
        "workload { arrive renewal(2); } session { end; } run { horizon 10; arrivals 1; }",
    );
    for command in ["run", "ir"] {
        let mut args = vec![command, "model.sq", "--arrivals", "3"];
        if command == "run" {
            args.push("--json");
        }
        let out = f.run(&args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(json["arrivals"], 3);
        for bad in ["0", "-1", "1.5", "abc"] {
            failure(
                &f.run(&[command, "model.sq", "--arrivals", bad]),
                2,
                &["--arrivals", "positive integer"],
            );
        }
    }
}

#[test]
fn arrival_override_also_applies_to_json_ir() {
    let f = Fixture::new();
    let program = compile_source(
        "workload { arrive renewal(2); } session { end; } run { horizon 10; arrivals 1; }",
        &Overrides::default(),
    )
    .unwrap();
    f.write("model.json", &program.to_json());
    for command in ["run", "ir"] {
        let mut args = vec![command, "model.json", "--arrivals", "3"];
        if command == "run" {
            args.push("--json");
        }
        let out = f.run(&args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(json["arrivals"], 3);
    }
}

/// An override reaches program text; IR has its constants folded and its
/// definitions expanded. The refusal speaks of the `let` or the `def`, not
/// of a CLI flag: pyserq's `sets=` and `defs=` meet the same error.
#[test]
fn overrides_are_refused_on_ir_by_what_they_override() {
    let f = Fixture::new();
    let program = compile_source(
        "let lam = 1; def law() = ~exp(1); workload { arrive poisson(lam); } \
         session { set x = law(); end; } run { horizon 10; }",
        &Overrides::default(),
    )
    .unwrap();
    f.write("model.json", &program.to_json());
    for command in ["run", "ir", "check", "draw"] {
        failure(
            &f.run(&[command, "model.json", "--set", "lam=2"]),
            1,
            &["a `let` override applies to program text, not to IR"],
        );
        failure(
            &f.run(&[command, "model.json", "--def", "law=~exp(2)"]),
            1,
            &["a `def` override applies to program text, not to IR"],
        );
    }
}
