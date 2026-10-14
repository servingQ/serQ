mod common;
use common::{Fixture, PROGRAM, WORKLOAD, failure};

#[test]
fn trace_errors_name_the_resolved_file_row_and_column() {
    let f = Fixture::new();
    let workload = WORKLOAD.replace(
        "arrive batch(1);",
        "arrive batch(1); trace \"data.csv\" ordered;",
    );
    f.write("models/model.sq", &common::main_source(PROGRAM));
    f.write("models/w.sq", &workload);
    f.write(
        "models/data.csv",
        "session,turn,new,out,think\n1,1,10,oops,0\n",
    );
    f.write("data.csv", "session,turn,new,out,think\n1,1,broken,1,0\n");
    for prefix in [
        vec![
            "run",
            "models/model.sq",
            "--workload",
            "models/w.sq",
            "--horizon",
            "10",
        ],
        vec![
            "ir",
            "models/model.sq",
            "--workload",
            "models/w.sq",
            "--horizon",
            "10",
            "--inline-trace",
        ],
    ] {
        failure(
            &f.run(&prefix),
            1,
            &[
                "trace models/data.csv",
                "line 2",
                "column 4 (out)",
                "`oops`",
                "help:",
            ],
        );
        let mut overridden = prefix;
        overridden.extend(["--trace", "data.csv"]);
        failure(
            &f.run(&overridden),
            1,
            &["trace data.csv", "line 2", "column 3 (new)", "`broken`"],
        );
    }
    f.write("data.csv", "session,turn,new,out,think\n1,1,10,1,0\n");
    assert!(
        f.run(&[
            "run",
            "models/model.sq",
            "--workload",
            "models/w.sq",
            "--horizon",
            "10",
            "--trace",
            "data.csv"
        ])
        .status
        .success()
    );
}

#[test]
fn empty_and_short_rows_explain_the_expected_schema() {
    let f = Fixture::new();
    f.write("model.sq", &common::main_source(PROGRAM));
    f.write(
        "w.sq",
        &WORKLOAD.replace(
            "arrive batch(1);",
            "arrive batch(1); trace \"data.csv\" ordered;",
        ),
    );
    for (csv, cause) in [
        ("# no rows\n", "empty trace"),
        ("1,1,10\n", "expected 5 or 6 fields, got 3"),
    ] {
        f.write("data.csv", csv);
        failure(
            &f.run(&["run", "model.sq", "--workload", "w.sq", "--horizon", "10"]),
            1,
            &[
                "trace data.csv",
                cause,
                "session,turn,new,out,think[,forced]",
            ],
        );
    }
}

/// The trace is the workload's: a relative path in the workload's file is
/// read beside that file, wherever the deployment is.
#[test]
fn a_workloads_trace_is_read_beside_the_workload() {
    let f = Fixture::new();
    f.write("deploy/model.sq", &common::main_source(PROGRAM));
    f.write(
        "load/w.sq",
        &WORKLOAD.replace(
            "arrive batch(1);",
            "arrive batch(1); trace \"data.csv\" ordered;",
        ),
    );
    f.write(
        "deploy/data.csv",
        "session,turn,new,out,think\n1,1,broken,1,0\n",
    );
    f.write(
        "load/data.csv",
        "session,turn,new,out,think\n1,1,10,oops,0\n",
    );
    for prefix in [
        vec![
            "run",
            "deploy/model.sq",
            "--workload",
            "load/w.sq",
            "--horizon",
            "10",
        ],
        vec![
            "ir",
            "deploy/model.sq",
            "--workload",
            "load/w.sq",
            "--horizon",
            "10",
            "--inline-trace",
        ],
    ] {
        failure(
            &f.run(&prefix),
            1,
            &["trace load/data.csv", "line 2", "column 4 (out)", "`oops`"],
        );
    }
    f.write("load/data.csv", "session,turn,new,out,think\n1,1,10,1,0\n");
    let out = f.run(&[
        "run",
        "deploy/model.sq",
        "--workload",
        "load/w.sq",
        "--horizon",
        "10",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
