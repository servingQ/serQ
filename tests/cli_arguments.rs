mod common;
use common::{Fixture, PROGRAM, WORKLOAD, failure};

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
            vec!["missing value for program argument `--bogus`"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--set", "rate=2"],
            vec!["invalid number `rate=2`", "--set", "--name value"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--=1"],
            vec!["invalid argument name", "identifier"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--rate="],
            vec!["invalid number ``", "--rate"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--", "--rate", "2"],
            vec!["unexpected `--`", "among the options"],
        ),
        (
            vec!["run", "missing.sq", "--horizon", "10", "--seed=2"],
            vec!["`seed` is the interpreter's option `--seed`"],
        ),
        (
            vec!["draw", "missing.sq", "--format", "png"],
            vec!["--format", "tikz or svg"],
        ),
        (
            vec!["draw", "missing.sq", "--view", "session"],
            vec!["invalid number `session`", "--view"],
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
    f.write("w.sq", WORKLOAD);
    for args in [
        vec![
            "run",
            "model.sq",
            "--workload",
            "w.sq",
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
        vec!["check", "model.sq", "--workload", "w.sq", "--rate", "2"],
        vec![
            "ir",
            "model.sq",
            "--workload",
            "w.sq",
            "--horizon",
            "10",
            "--seed",
            "2",
        ],
        vec![
            "draw",
            "model.sq",
            "--workload",
            "w.sq",
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
        &["missing value for program argument `--version`"],
    );
}

/// `--workload W` gives the deployment its workload's file on every command
/// that compiles program text (`docs/design/paired-workload.md`); a server
/// given none is refused.
#[test]
fn the_workload_is_given_on_every_command_that_compiles() {
    let f = Fixture::new();
    f.write("model.sq", &common::main_source(PROGRAM));
    f.write("w.sq", WORKLOAD);
    for args in [
        vec!["run", "model.sq", "--workload", "w.sq", "--horizon", "10"],
        vec!["check", "model.sq", "--workload", "w.sq"],
        vec!["ir", "model.sq", "--workload", "w.sq", "--horizon", "10"],
        vec!["draw", "model.sq", "--workload", "w.sq"],
    ] {
        let out = f.run(&args);
        assert!(
            out.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        let mut alone = args.clone();
        alone.drain(2..4);
        failure(
            &f.run(&alone),
            1,
            &["model.sq", "the server needs a workload to run"],
        );
    }
    let root = env!("CARGO_MANIFEST_DIR");
    let vllm = format!("{root}/examples/multi-turn/vllm.sq");
    let chat = format!("{root}/examples/workloads/chat.sq");
    let out = f.run(&["target", &vllm, "--workload", &chat]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap();
}

/// IR holds the workload it was compiled with: another one is refused, as a
/// usage error.
#[test]
fn a_workload_is_refused_with_ir() {
    let f = Fixture::new();
    f.write("model.sq", &common::main_source(PROGRAM));
    f.write("w.sq", WORKLOAD);
    let out = f.run(&["ir", "model.sq", "--workload", "w.sq", "--horizon", "10"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    f.write("model.json", &String::from_utf8(out.stdout).unwrap());
    for command in ["run", "ir", "check", "draw", "target"] {
        failure(
            &f.run(&[command, "model.json", "--workload", "w.sq"]),
            2,
            &[
                "--workload pairs a .sq deployment with a workload file; an IR is a whole program already",
            ],
        );
    }
}

#[test]
fn a_missing_workload_file_is_named() {
    let f = Fixture::new();
    f.write("model.sq", &common::main_source(PROGRAM));
    for command in ["run", "check", "ir", "draw", "target"] {
        failure(
            &f.run(&[command, "model.sq", "--workload", "absent.sq"]),
            1,
            &["cannot read absent.sq"],
        );
    }
}

/// An error in the workload's file is located in that file, by its own
/// lines, and not prefixed with the deployment's path.
#[test]
fn an_error_in_the_workload_is_located_in_its_file() {
    let f = Fixture::new();
    f.write("model.sq", &common::main_source(PROGRAM));
    f.write(
        "w.sq",
        "workload { arrive batch(1);\n  init { set c = nowhere; }\n  session { turn; end; } }\n",
    );
    for command in ["run", "check", "ir", "draw"] {
        let out = f.run(&[command, "model.sq", "--workload", "w.sq"]);
        failure(&out, 1, &["nowhere"]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.starts_with("w.sq: 2:18:"), "{stderr}");
        assert!(!stderr.contains("model.sq"), "{stderr}");
    }
}

/// `serq draw --log LOG` draws the log `run --log` wrote (the iteration
/// view), and takes neither a program nor a run's options.
#[test]
fn a_run_s_log_is_drawn() {
    let fx = Fixture::new();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let program = root.join("docs/tutorial/programs/05-engine.sq");
    let workload = root.join("docs/tutorial/programs/workloads/05-engine.sq");
    let out = fx.run(&[
        "run",
        program.to_str().unwrap(),
        "--workload",
        workload.to_str().unwrap(),
        "--horizon",
        "10",
        "--log",
        "l.json",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = fx.run(&[
        "draw",
        "--log",
        "l.json",
        "--from",
        "0",
        "--iterations",
        "20",
        "--format",
        "svg",
        "--out",
        "f.svg",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let svg = std::fs::read_to_string(fx.0.join("f.svg")).unwrap();
    assert!(
        svg.starts_with("<svg") && svg.contains("class=\"prefill\""),
        "{svg}"
    );
    // TikZ, to stdout, by default
    let out = fx.run(&["draw", "--log", "l.json", "--engine", "llm"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("\\begin{tikzpicture}"));
    for (args, fragments) in [
        (
            vec!["draw", "p.sq", "--log", "l.json"],
            vec!["takes no FILE", "`p.sq`"],
        ),
        (
            vec!["draw", "--log", "l.json", "--seed", "1"],
            vec!["--seed does not apply to `draw --log`"],
        ),
        (vec!["draw", "--log"], vec!["missing value for --log"]),
        (
            vec!["draw", "--log", "l.json", "--iterations", "0"],
            vec!["--iterations", "positive integer"],
        ),
        (
            vec!["draw", "--log", "l.json", "--from", "-1"],
            vec!["--from", "nonnegative"],
        ),
        (
            vec!["draw", "--log", "l.json", "--format", "png"],
            vec!["invalid --format `png`"],
        ),
    ] {
        failure(&fx.run(&args), 2, &fragments);
    }
    // a window past the end, an engine the log does not have, a file
    // that is no log: the file's error
    for (args, fragments) in [
        (
            vec!["draw", "--log", "l.json", "--from", "100"],
            vec!["l.json: `llm` has no iteration from t = 100"],
        ),
        (
            vec!["draw", "--log", "l.json", "--engine", "E"],
            vec!["no stage `E`", "llm"],
        ),
        (
            vec!["draw", "--log", "missing.json"],
            vec!["missing.json: cannot read"],
        ),
    ] {
        failure(&fx.run(&args), 1, &fragments);
    }
    fx.write("ir.json", "{\"version\": 15, \"stages\": []}");
    failure(
        &fx.run(&["draw", "--log", "ir.json"]),
        1,
        &["not a run log"],
    );
}
