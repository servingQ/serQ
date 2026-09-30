mod common;
use common::{Fixture, PROGRAM, failure};

#[test]
fn invalid_arguments_are_diagnosed_before_file_io() {
    let f = Fixture::new();
    for (args, fragments) in [
        (
            vec!["rn", "missing.serq"],
            vec!["unknown command `rn`", "run"],
        ),
        (vec!["run"], vec!["missing FILE"]),
        (
            vec!["run", "missing.serq", "--seed", "abc"],
            vec!["--seed", "`abc`", "unsigned integer"],
        ),
        (
            vec!["run", "missing.serq", "--seed"],
            vec!["missing value for --seed"],
        ),
        (
            vec!["run", "missing.serq", "--seed", "--json"],
            vec!["missing value for --seed"],
        ),
        (
            vec!["run", "missing.serq", "--seed", "-1"],
            vec!["--seed", "unsigned integer"],
        ),
        (
            vec!["run", "missing.serq", "--horizon", "NaN"],
            vec!["--horizon", "finite positive"],
        ),
        (
            vec!["run", "missing.serq", "--warmup", "-1"],
            vec!["--warmup", "nonnegative"],
        ),
        (
            vec!["run", "missing.serq", "--bogus"],
            vec!["unknown option `--bogus`"],
        ),
        (
            vec!["run", "missing.serq", "--set", "rate"],
            vec!["--set", "name=expr"],
        ),
        (
            vec!["run", "missing.serq", "--set", "=1"],
            vec!["--set", "identifier"],
        ),
        (
            vec!["run", "missing.serq", "--set", "rate="],
            vec!["--set", "expression"],
        ),
        (
            vec!["draw", "missing.serq", "--format", "png"],
            vec!["--format", "tikz or svg"],
        ),
        (
            vec!["draw", "missing.serq", "--view", "session"],
            vec!["unknown option `--view`"],
        ),
        (
            vec!["check", "missing.serq", "--json"],
            vec!["--json", "not supported", "run"],
        ),
        (
            vec!["run", "missing.serq", "--inline-trace"],
            vec!["--inline-trace", "not supported", "ir"],
        ),
        (
            vec!["ir", "missing.serq", "--dump", "data"],
            vec!["--dump", "not supported", "run"],
        ),
        (
            vec!["run", "missing.serq", "--out", "figure.svg"],
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
    f.write("model.serq", PROGRAM);
    for args in [
        vec![
            "run",
            "model.serq",
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
        vec!["check", "model.serq", "--set", "rate=2"],
        vec!["ir", "model.serq", "--seed", "2"],
        vec![
            "draw",
            "model.serq",
            "--format",
            "svg",
            "--out",
            "figure.svg",
        ],
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
