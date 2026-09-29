mod common;
use common::{Fixture, PROGRAM, failure};

#[test]
fn invalid_arguments_are_diagnosed_before_file_io() {
    let f = Fixture::new();
    for (args, fragments) in [
        (
            vec!["rn", "missing.seq"],
            vec!["unknown command `rn`", "run"],
        ),
        (vec!["run"], vec!["missing FILE"]),
        (
            vec!["run", "missing.seq", "--seed", "abc"],
            vec!["--seed", "`abc`", "unsigned integer"],
        ),
        (
            vec!["run", "missing.seq", "--seed"],
            vec!["missing value for --seed"],
        ),
        (
            vec!["run", "missing.seq", "--seed", "--json"],
            vec!["missing value for --seed"],
        ),
        (
            vec!["run", "missing.seq", "--seed", "-1"],
            vec!["--seed", "unsigned integer"],
        ),
        (
            vec!["run", "missing.seq", "--horizon", "NaN"],
            vec!["--horizon", "finite positive"],
        ),
        (
            vec!["run", "missing.seq", "--warmup", "-1"],
            vec!["--warmup", "nonnegative"],
        ),
        (
            vec!["run", "missing.seq", "--bogus"],
            vec!["unknown option `--bogus`"],
        ),
        (
            vec!["run", "missing.seq", "--set", "rate"],
            vec!["--set", "name=expr"],
        ),
        (
            vec!["run", "missing.seq", "--set", "=1"],
            vec!["--set", "identifier"],
        ),
        (
            vec!["run", "missing.seq", "--set", "rate="],
            vec!["--set", "expression"],
        ),
        (
            vec!["draw", "missing.seq", "--format", "png"],
            vec!["--format", "tikz or svg"],
        ),
        (
            vec!["draw", "missing.seq", "--view", "nope"],
            vec!["--view", "deployment or session"],
        ),
        (
            vec!["check", "missing.seq", "--json"],
            vec!["--json", "not supported", "run"],
        ),
        (
            vec!["run", "missing.seq", "--inline-trace"],
            vec!["--inline-trace", "not supported", "ir"],
        ),
        (
            vec!["ir", "missing.seq", "--dump", "data"],
            vec!["--dump", "not supported", "run"],
        ),
        (
            vec!["run", "missing.seq", "--out", "figure.svg"],
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
    f.write("model.seq", PROGRAM);
    for args in [
        vec![
            "run",
            "model.seq",
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
        vec!["check", "model.seq", "--set", "rate=2"],
        vec!["ir", "model.seq", "--seed", "2"],
        vec![
            "draw",
            "model.seq",
            "--view",
            "session",
            "--format",
            "svg",
            "--out",
            "figure.svg",
            "--show-set",
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
