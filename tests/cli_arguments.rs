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
            vec!["run", "missing.sq", "--seed", "abc"],
            vec!["--seed", "`abc`", "unsigned integer"],
        ),
        (
            vec!["run", "missing.sq", "--seed"],
            vec!["missing value for --seed"],
        ),
        (
            vec!["run", "missing.sq", "--seed", "--json"],
            vec!["missing value for --seed"],
        ),
        (
            vec!["run", "missing.sq", "--seed", "-1"],
            vec!["--seed", "unsigned integer"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "NaN"],
            vec!["--horizon", "finite positive"],
        ),
        (
            vec!["run", "missing.sq", "--warmup", "-1"],
            vec!["--warmup", "nonnegative"],
        ),
        (
            vec!["run", "missing.sq", "--bogus"],
            vec!["unknown option `--bogus`"],
        ),
        (
            vec!["run", "missing.sq", "--set", "rate"],
            vec!["--set", "name=expr"],
        ),
        (
            vec!["run", "missing.sq", "--set", "=1"],
            vec!["--set", "identifier"],
        ),
        (
            vec!["run", "missing.sq", "--set", "rate="],
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
            vec!["run", "missing.sq", "--inline-trace"],
            vec!["--inline-trace", "not supported", "ir"],
        ),
        (
            vec!["ir", "missing.sq", "--dump", "data"],
            vec!["--dump", "not supported", "run"],
        ),
        (
            vec!["run", "missing.sq", "--out", "figure.svg"],
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
    f.write("model.sq", PROGRAM);
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
        vec!["ir", "model.sq", "--seed", "2"],
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
