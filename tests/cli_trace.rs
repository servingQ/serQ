mod common;
use common::{Fixture, PROGRAM, failure};

#[test]
fn trace_errors_name_the_resolved_file_row_and_column() {
    let f = Fixture::new();
    let program = PROGRAM.replace(
        "arrive batch(1);",
        "arrive batch(1); trace \"data.csv\" ordered;",
    );
    f.write("models/model.serq", &program);
    f.write(
        "models/data.csv",
        "session,turn,new,out,think\n1,1,10,oops,0\n",
    );
    f.write("data.csv", "session,turn,new,out,think\n1,1,broken,1,0\n");
    for prefix in [
        vec!["run", "models/model.serq"],
        vec!["ir", "models/model.serq", "--inline-trace"],
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
        f.run(&["run", "models/model.serq", "--trace", "data.csv"])
            .status
            .success()
    );
}

#[test]
fn empty_and_short_rows_explain_the_expected_schema() {
    let f = Fixture::new();
    f.write(
        "model.serq",
        &PROGRAM.replace(
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
            &f.run(&["run", "model.serq"]),
            1,
            &[
                "trace data.csv",
                cause,
                "session,turn,new,out,think[,forced]",
            ],
        );
    }
}
