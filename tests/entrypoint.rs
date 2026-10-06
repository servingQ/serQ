//! The source entry point and its explicit external input boundary.
mod common;
use common::Fixture;
use serq::compile_source;

const SOURCE: &str = r#"
use "std/args";
let fixed = 2;
def twice(x) { x * 2 }
fn main() {
  let rate = args.number("arrival_rate", 0.5);
  let service = twice(fixed);
  stage worker : delay;
  workload { arrive batch(1); session { turn; end; } }
  server { run worker (service); observe elapsed = now; }

}
"#;

#[test]
fn main_constructs_the_program_and_only_its_session_runs_per_arrival() {
    let r = serq::run_source(SOURCE, &common::horizon(10.0), None).unwrap();
    // One request, one delay of twice(2) seconds, regardless of the input name.
    assert_eq!(r.observe("elapsed").unwrap().mean, 4.0);
    let p = compile_source(SOURCE, &common::horizon(10.0)).unwrap();
    assert_eq!(p.horizon, 10.0);
}

#[test]
fn program_inputs_are_explicit_and_can_have_different_local_names() {
    let src = SOURCE.replace("arrive batch(1)", "arrive poisson(rate * fixed)");
    let mut ov = common::horizon(10.0);
    ov.set("arrival_rate", "3").unwrap();
    let p = compile_source(&src, &ov).unwrap();
    assert_eq!(p.arrival, serq::ir::CArrival::Poisson(6.0));
    assert_eq!(
        compile_source(&src, &common::horizon(10.0))
            .unwrap()
            .arrival,
        serq::ir::CArrival::Poisson(1.0)
    );
    for private in ["fixed", "rate", "service"] {
        let mut ov = common::horizon(10.0);
        ov.set(private, "10").unwrap();
        assert!(
            compile_source(SOURCE, &ov)
                .unwrap_err()
                .contains("unknown program argument")
        );
    }
}

#[test]
fn absent_duplicate_nested_and_implicit_entry_points_are_rejected() {
    for (source, message) in [
        ("let x = 1;", "missing `fn main()`"),
        ("pool slots { cap 1; }", "inside `fn main()"),
        ("fn main() {} fn main() {}", "exactly one"),
        ("fn main() { fn main() {} }", "exactly one"),
        ("fn worker() {}", "entry point is `fn main()`"),
        ("fn main(x) {}", "expected RParen"),
        ("fn main() {", "unclosed"),
        ("fn main() {} let x = 1;", "before `fn main()`"),
    ] {
        let err = compile_source(source, &common::horizon(1.0)).unwrap_err();
        assert!(err.contains(message), "{source}: {err}");
    }
}

#[test]
fn argument_declarations_require_the_library_and_main() {
    for src in [
        "fn main() { let x = args.number(\"x\", 1); }",
        "use \"std/args\"; let x = args.number(\"x\", 1); fn main() {}",
        "use \"std/args\"; fn main() { let x = args.number(\"x\", 1); let y = args.number(\"x\", 2); }",
        "use \"std/args\"; fn main() { let x = args.number(\"not a name\", 1); }",
        "use \"std/args\"; fn main() { let args = 1; }",
    ] {
        assert!(
            compile_source(src, &common::horizon(10.0)).is_err(),
            "{src}"
        );
    }
}

#[test]
fn cli_and_instance_bind_the_same_declared_inputs() {
    let f = Fixture::new();
    f.write(
        "model.sq",
        &SOURCE.replace("arrive batch(1)", "arrive poisson(rate)"),
    );
    f.write("values.sq", "let arrival_rate = 3;");
    let direct = f.run(&[
        "ir",
        "model.sq",
        "--horizon",
        "10",
        "--",
        "--arrival_rate",
        "3",
    ]);
    let instance = f.run(&[
        "ir",
        "model.sq",
        "--horizon",
        "10",
        "--instance",
        "values.sq",
    ]);
    let set = f.run(&[
        "ir",
        "model.sq",
        "--horizon",
        "10",
        "--set",
        "arrival_rate=3",
    ]);
    assert!(
        direct.status.success(),
        "{}",
        String::from_utf8_lossy(&direct.stderr)
    );
    assert_eq!(direct.stdout, instance.stdout);
    assert_eq!(direct.stdout, set.stdout);
    for rest in [
        vec!["--fixed", "3"],
        vec!["--arrival_rate", "NaN"],
        vec!["--arrival_rate"],
        vec!["oops"],
    ] {
        let mut command = vec!["ir", "model.sq", "--horizon", "10", "--"];
        command.extend(rest);
        assert!(!f.run(&command).status.success());
    }
}

#[test]
fn an_input_name_does_not_make_an_unrelated_constant_structural() {
    let src = r#"use "std/args"; fn main() {
      let count = 2;
      let cost = args.number("count", 1);
      queue q[count] : prefill { serve delay; prefill(p) { run (p); } }
      workload { session { turn; end; } }
      server { q[0].prefill(cost); }

    }"#;
    let mut ov = common::horizon(10.0);
    ov.set("count", "3").unwrap();
    compile_source(src, &ov).unwrap();
}

#[test]
fn importing_a_program_cannot_execute_its_main() {
    let f = Fixture::new();
    f.write("other.sq", "fn main() {  }");
    f.write("model.sq", "use \"other.sq\"; fn main() {  }");
    let out = f.run(&["check", "model.sq"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("a library holds definitions"));
}

#[test]
fn numeric_options_accept_equals_negatives_and_last_value() {
    let f = Fixture::new();
    f.write("model.sq", SOURCE);
    // This input is unused in this fixture; accepting -2 tests numeric parsing
    // without asking an arrival process to have a negative rate.
    let out = f.run(&[
        "ir",
        "model.sq",
        "--horizon",
        "10",
        "--",
        "--arrival_rate=-2",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    f.write(
        "model.sq",
        &SOURCE.replace("arrive batch(1)", "arrive poisson(rate)"),
    );
    let out = f.run(&[
        "ir",
        "model.sq",
        "--horizon",
        "10",
        "--",
        "--arrival_rate",
        "2",
        "--arrival_rate=3",
    ]);
    assert!(out.status.success());
    let p: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(p["arrival"]["Poisson"], 3.0);
}

#[test]
fn inputs_bind_declarations_without_replacing_shadowed_constants() {
    for (declarations, expected) in [
        ("let x = args.number(\"first\", 1); let x = 7;", 7.0),
        (
            "let x = 2; let saved = x * 2; let x = args.number(\"first\", 1); let x = saved + x;",
            7.0,
        ),
        (
            "let x = args.number(\"first\", 1); let saved = x; let x = args.number(\"second\", 2); let x = saved + x;",
            8.0,
        ),
    ] {
        let src = format!(
            "use \"std/args\"; fn main() {{ {declarations} workload {{ arrive poisson(x); }} server {{}}  }}"
        );
        let mut ov = common::horizon(1.0);
        ov.set("first", "3").unwrap();
        if declarations.contains("second") {
            ov.set("second", "5").unwrap();
        }
        // The last fixed binding is 7; an earlier fixed x contributes 2*2;
        // two distinct inputs contribute 3+5 even with one local spelling.
        assert_eq!(
            compile_source(&src, &ov).unwrap().arrival,
            serq::ir::CArrival::Poisson(expected)
        );
    }
}

#[test]
fn a_fixed_shadow_can_size_an_array_but_an_input_dependent_shadow_cannot() {
    let src = r#"use "std/args"; fn main() {
        let count = args.number("size", 2);
        let count = 3;
        stage workers[count] : delay;

    }"#;
    let mut ov = common::horizon(1.0);
    ov.set("size", "4").unwrap();
    assert_eq!(compile_source(src, &ov).unwrap().stages.len(), 3);
    let dependent = src.replace("let count = 3;", "let count = count + 1;");
    assert!(
        compile_source(&dependent, &ov)
            .unwrap_err()
            .contains("affects an array size")
    );
}
