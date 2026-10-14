mod common;
use common::{Fixture, PROGRAM, WORKLOAD, failure};
use serq::frontend::lexer::{lex, unlex};

#[test]
fn unterminated_comments_point_to_the_opening_delimiter() {
    for tail in ["/*", "/* one\ntwo", "/* *"] {
        let src = format!("// 한글\n  {tail}");
        let err = lex(&src).unwrap_err();
        assert_eq!((err.line, err.col), (2, 3));
        assert!(err.msg.contains("unterminated block comment"));
        assert!(err.msg.contains("`*/`"));
    }
    for src in ["/* ok */", "// /*", "\"/*\"", "/* one\ntwo */"] {
        assert_eq!(unlex(&lex(src).unwrap()), src);
    }
}

#[test]
fn every_command_rejects_incomplete_comments_without_an_artifact() {
    let f = Fixture::new();
    f.write(
        "bad.sq",
        &common::main_source(&format!("{PROGRAM}/* unfinished\nignored\n")),
    );
    f.write("w.sq", WORKLOAD);
    for cmd in ["check", "run", "ir", "draw"] {
        failure(
            &f.run(&[cmd, "bad.sq", "--workload", "w.sq"]),
            1,
            &["bad.sq", "5:1", "unterminated block comment", "`*/`"],
        );
    }
    failure(
        &f.run(&[
            "draw",
            "bad.sq",
            "--workload",
            "w.sq",
            "--out",
            "figure.svg",
        ]),
        1,
        &["unterminated block comment"],
    );
    assert!(!f.0.join("figure.svg").exists());
    f.write(
        "good.sq",
        &common::main_source(&format!("{PROGRAM}/* finished\nignored\n*/")),
    );
    assert!(
        f.run(&["check", "good.sq", "--workload", "w.sq"])
            .status
            .success()
    );
    // a comment left open in the workload's file is that file's error
    f.write("open.sq", &format!("{WORKLOAD}/* unfinished\n"));
    failure(
        &f.run(&["check", "good.sq", "--workload", "open.sq"]),
        1,
        &["open.sq: 2:1", "unterminated block comment"],
    );
}
