mod common;
use common::{Fixture, PROGRAM, failure};
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
    for cmd in ["check", "run", "ir", "draw"] {
        failure(
            &f.run(&[cmd, "bad.sq"]),
            1,
            &["bad.sq", "6:1", "unterminated block comment", "`*/`"],
        );
    }
    failure(
        &f.run(&["draw", "bad.sq", "--out", "figure.svg"]),
        1,
        &["unterminated block comment"],
    );
    assert!(!f.0.join("figure.svg").exists());
    f.write(
        "good.sq",
        &common::main_source(&format!("{PROGRAM}/* finished\nignored\n*/")),
    );
    assert!(f.run(&["check", "good.sq"]).status.success());
}
