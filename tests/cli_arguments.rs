mod common;
use common::{Fixture, PROGRAM, failure};

#[test]
fn invalid_arguments_are_diagnosed_before_file_io() {
    let f = Fixture::new();
    for (args, fragments) in [
        (
            vec!["rn", "missing.sq"],
            vec!["unknown command `rn`", "run"],
        ),
        (vec!["run"], vec!["missing FILE"]),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--seed", "abc"],
            vec!["--seed", "`abc`", "unsigned integer"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--seed"],
            vec!["missing value for --seed"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--seed", "--json"],
            vec!["missing value for --seed"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--seed", "-1"],
            vec!["--seed", "unsigned integer"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--horizon", "NaN"],
            vec!["--horizon", "finite positive"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--warmup", "-1"],
            vec!["--warmup", "nonnegative"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--bogus"],
            vec!["unknown option `--bogus`"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--set", "rate"],
            vec!["--set", "name=expr"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--set", "=1"],
            vec!["--set", "identifier"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--set", "rate="],
            vec!["--set", "expression"],
        ),
        (
            vec!["draw", "missing.sq", "--format", "png"],
            vec!["--format", "tikz or svg"],
        ),
        (
            vec!["draw", "missing.sq", "--view", "session"],
            vec!["unknown option `--view`"],
        ),
        (
            vec!["check", "missing.sq", "--json"],
            vec!["--json", "not supported", "run"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--inline-trace"],
            vec!["--inline-trace", "not supported", "ir"],
        ),
        (
            vec!["ir", "missing.sq", "--horizon", "10", "--dump", "data"],
            vec!["--dump", "not supported", "run"],
        ),
        (
            vec![
                "run",
                "missing.sq",
                "--horizon",
                "10",
                "--out",
                "figure.svg",
            ],
            vec!["--out", "not supported", "draw"],
        ),
    ] {
        let output = f.run(&args);
        failure(&output, 2, &fragments);
        let err = String::from_utf8_lossy(&output.stderr);
        assert!(err.contains("help:"), "{err}");
        assert!(!err.contains("cannot read"), "{err}");
    }
    assert!(!f.0.join("figure.svg").exists());
    assert!(!f.0.join("data").exists());
}

#[test]
fn supported_options_still_work() {
    let f = Fixture::new();
    f.write("model.sq", &common::main_source(PROGRAM));
    for args in [
        vec![
            "run",
            "model.sq",
            "--seed",
            "2",
            "--horizon",
            "20",
            "--warmup",
            "0",
            "--json",
            "--dump",
            "data",
        ],
        vec!["check", "model.sq", "--set", "rate=2"],
        vec!["ir", "model.sq", "--horizon", "10", "--seed", "2"],
        vec!["draw", "model.sq", "--format", "svg", "--out", "figure.svg"],
    ] {
        let out = f.run(&args);
        assert!(
            out.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert!(f.0.join("figure.svg").exists());
}

/// #232: `serq --version` prints the crate's version, for the record of a
/// run; `-V` is the same, and either with anything else is an error.
#[test]
fn version_is_printed_on_request() {
    let f = Fixture::new();
    for flag in ["--version", "-V"] {
        let out = f.run(&[flag]);
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("serq {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
    failure(
        &f.run(&["--version", "x.sq"]),
        2,
        &["`--version` takes no arguments, found `x.sq`"],
    );
    failure(
        &f.run(&["run", "x.sq", "--horizon", "10", "--version"]),
        2,
        &["unknown option `--version`"],
    );
}
