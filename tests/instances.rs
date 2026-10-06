//! `--instance FILE`: the values of a program's constants and its run
//! options, in a file of their own. An instance is the `--set`s and run
//! flags it is equivalent to, so the IR it gives is one they give.

mod common;
use common::{Fixture, PROGRAM, failure};
use serq::compile_source;

#[test]
fn an_instance_is_the_sets_and_flags_it_writes() {
    let f = Fixture::new();
    f.write("model.sq", &common::main_source(PROGRAM));
    f.write(
        "fast.sq",
        "let rate = 0.5;\nrun { horizon 20; warmup 2; seed 7; }\n",
    );
    let by_instance = f.run(&["ir", "model.sq", "--horizon", "10", "--instance", "fast.sq"]);
    let by_flags = f.run(&[
        "ir",
        "model.sq",
        "--set",
        "rate=0.5",
        "--horizon",
        "20",
        "--warmup",
        "2",
        "--seed",
        "7",
    ]);
    assert!(
        by_instance.status.success(),
        "{}",
        String::from_utf8_lossy(&by_instance.stderr)
    );
    assert_eq!(by_instance.stdout, by_flags.stdout);
    let json: serde_json::Value = serde_json::from_slice(&by_instance.stdout).unwrap();
    assert_eq!(json["horizon"], 20.0);
    assert_eq!(json["seed"], 7);
}

#[test]
fn the_later_of_an_instance_and_a_flag_wins() {
    let f = Fixture::new();
    f.write("model.sq", &common::main_source(PROGRAM));
    f.write("i.sq", "let rate = 2;\nrun { seed 5; }\n");
    let seed = |args: &[&str]| -> serde_json::Value {
        let out = f.run(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };
    assert_eq!(
        seed(&[
            "ir",
            "model.sq",
            "--horizon",
            "10",
            "--instance",
            "i.sq",
            "--seed",
            "9"
        ])["seed"],
        9
    );
    assert_eq!(
        seed(&[
            "ir",
            "model.sq",
            "--horizon",
            "10",
            "--seed",
            "9",
            "--instance",
            "i.sq"
        ])["seed"],
        5
    );
    let mut ov = common::horizon(10.0);
    ov.instance("let rate = 2;").unwrap();
    ov.set("rate", "3").unwrap();
    let p = compile_source(&common::main_source(PROGRAM), &ov).unwrap();
    let mut by_set = common::horizon(10.0);
    by_set.set("rate", "3").unwrap();
    assert_eq!(
        p.to_json(),
        compile_source(&common::main_source(PROGRAM), &by_set)
            .unwrap()
            .to_json()
    );
}

#[test]
fn an_instance_changes_values_not_structure() {
    for (text, fragment) in [
        (
            "pool kv { cap 1; }",
            "an instance binds values: found `pool`",
        ),
        ("def twice(x) { 2 * x }", "found `def`"),
        ("use \"lib.sq\";", "found `use`"),
        (
            "let rate = 1; let rate = 2;",
            "`rate` is bound twice in this instance",
        ),
        ("run { seed 1; } run { seed 2; }", "found `run`"),
        (
            "run { horizon rate; }",
            "the instance's run option `horizon` is not a number",
        ),
        (
            "run { seed 1.5; }",
            "the instance's seed 1.5 is not an unsigned integer",
        ),
        (
            "run { horizon 0; }",
            "the instance's horizon 0 is not a finite positive number",
        ),
    ] {
        let err = common::horizon(10.0).instance(text).unwrap_err();
        assert!(err.contains(fragment), "{text}: {err}");
    }
}

#[test]
fn an_instance_names_only_declared_constants() {
    let f = Fixture::new();
    f.write("model.sq", &common::main_source(PROGRAM));
    f.write("typo.sq", "let raet = 2;\n");
    failure(
        &f.run(&["check", "model.sq", "--instance", "typo.sq"]),
        1,
        &[
            "unknown program argument `raet`",
            "available arguments: rate",
        ],
    );
    failure(
        &f.run(&["check", "model.sq", "--instance", "missing.sq"]),
        1,
        &["missing.sq", "cannot read"],
    );
    failure(
        &f.run(&["fmt", "model.sq", "--instance", "typo.sq"]),
        2,
        &[],
    );
}

/// `examples/<dir>/instances/<program>/<name>.sq` is an instance of
/// `examples/<dir>/<program>.sq`, and links with it.
#[test]
fn example_instances_link_with_their_programs() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut n = 0;
    for dir in std::fs::read_dir(&root).unwrap() {
        let instances = dir.unwrap().path().join("instances");
        let Ok(programs) = std::fs::read_dir(&instances) else {
            continue;
        };
        for program in programs {
            let program = program.unwrap().path();
            let source = instances
                .parent()
                .unwrap()
                .join(program.file_name().unwrap())
                .with_extension("sq");
            for instance in std::fs::read_dir(&program).unwrap() {
                let instance = instance.unwrap().path();
                let mut ov = serq::Overrides::default();
                ov.instance(&std::fs::read_to_string(&instance).unwrap())
                    .unwrap_or_else(|e| panic!("{}: {e}", instance.display()));
                serq::load(&source, &ov).unwrap_or_else(|e| panic!("{}: {e}", instance.display()));
                n += 1;
            }
        }
    }
    assert!(n > 0, "no example instances found");
}
