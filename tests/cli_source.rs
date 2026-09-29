mod common;
use common::{Fixture, PROGRAM, failure};
use seq::{Overrides, compile_source, parser};

#[test]
fn unknown_references_show_the_use_and_a_declaration_of_the_right_kind() {
    let f = Fixture::new();
    f.write(
        "model.seq",
        &PROGRAM.replace("run svc (rate)", "run svcc (rate)"),
    );
    failure(
        &f.run(&["check", "model.seq"]),
        1,
        &[
            "model.seq: 4:15:",
            "unknown stage `svcc`",
            "4 | session { run svcc (rate); end; }",
            "^^^^",
            "did you mean stage `svc`?",
            "declared at 2:7",
        ],
    );
    // The first spelling is a valid stage, but the second is an unknown pool.
    // Searching the token stream for the first matching name would misdiagnose it.
    let src = "stage kvv : fifo;\npool kv { cap 10; }\nsession { hold kvv (1) { end; } }\nrun { horizon 10; }";
    f.write("model.seq", src);
    failure(
        &f.run(&["check", "model.seq"]),
        1,
        &[
            "3:16:",
            "unknown pool `kvv`",
            "did you mean pool `kv`?",
            "declared at 2:6",
        ],
    );
}

#[test]
fn names_and_bare_references_keep_their_locations() {
    for (src, location, cause) in [
        (
            "session { set x = min(typo, 1); }\nrun { horizon 1; }",
            "1:23:",
            "unknown name `typo`",
        ),
        (
            "pool kv { admit via engin; }\nstage engine : step { cost 1; }\nrun { horizon 1; }",
            "1:21:",
            "unknown stage `engin`",
        ),
        (
            "pool kv { cap 10; }\nstage engine : step { memory kvv; cost 1; }\nrun { horizon 1; }",
            "2:30:",
            "unknown pool `kvv`",
        ),
        (
            "// 한글 주석\nlet 용량 = 10;\nsession { set x = 용랑; }\nrun { horizon 1; }",
            "3:19:",
            "unknown name `용랑`",
        ),
        (
            "let rate = 1;\nlet x = min(raet, 1);\nrun { horizon 1; }",
            "2:13:",
            "`raet` is not a constant",
        ),
    ] {
        let err = compile_source(src, &Overrides::default()).unwrap_err();
        assert!(err.contains(location), "{err}");
        assert!(err.contains(cause), "{err}");
        assert!(err.contains("help:"), "{err}");
    }
}

#[test]
fn duplicate_declarations_point_to_both_sites() {
    for (src, first, second) in [
        (
            "stage svc : fifo;\nstage svc : delay;\nrun { horizon 1; }",
            "1:7",
            "2:7:",
        ),
        (
            "pool kv { cap 1; }\npool kv { cap 2; }\nrun { horizon 1; }",
            "1:6",
            "2:6:",
        ),
    ] {
        let err = compile_source(src, &Overrides::default()).unwrap_err();
        assert!(err.contains(second), "{err}");
        assert!(err.contains(&format!("first declared at {first}")), "{err}");
    }
}

#[test]
fn desugaring_keeps_server_and_header_binding_locations() {
    let src = "stage svc : fifo;\nworkload { arrive batch(1); session { request; end; } }\nserver {\n  run svcc (1);\n}\nrun { horizon 10; }";
    let err = compile_source(src, &Overrides::default()).unwrap_err();
    assert!(err.contains("4:7:"), "{err}");
    assert!(err.contains("4 |   run svcc (1);"), "{err}");
    let src = "pool kv { cap 10; }\nworkload { arrive batch(1); session { request; end; } }\nserver {\n  admit if kv (amount) fit where amount = missing { }\n}\nrun { horizon 10; }";
    let err = compile_source(src, &Overrides::default()).unwrap_err();
    // `missing`, in the binding, is where the failed expression was written.
    assert!(err.contains("4:43:"), "{err}");
    assert!(err.contains("unknown name `missing`"), "{err}");
}

#[test]
fn ambiguous_suggestions_and_override_spans_are_not_misleading() {
    let src = "stage cat : fifo; stage cut : fifo; session { run cot (1); } run { horizon 1; }";
    let err = compile_source(src, &Overrides::default()).unwrap_err();
    assert!(!err.contains("did you mean"), "{err}");
    let ov = Overrides {
        lets: vec![("rate".into(), parser::parse_expr("missing").unwrap())],
        ..Default::default()
    };
    let err = compile_source(PROGRAM, &ov).unwrap_err();
    assert!(err.contains("--set rate"), "{err}");
    assert!(
        !err.contains("1 |"),
        "an override is not line 1 of the program: {err}"
    );
}

#[test]
fn eof_errors_keep_the_eof_location_and_never_panic() {
    let err = compile_source("run { horizon 1;", &Overrides::default()).unwrap_err();
    assert!(err.contains("1:17:"), "{err}");
    let err = parser::parse_expr("").unwrap_err();
    assert_eq!((err.line, err.col), (1, 1));
    let err = compile_source("stage svc[", &Overrides::default()).unwrap_err();
    assert!(err.contains("1:11:"), "{err}");
}

#[test]
fn ir_errors_use_ir_context_instead_of_a_fabricated_source_location() {
    let f = Fixture::new();
    let mut p = compile_source(PROGRAM, &Overrides::default()).unwrap();
    p.stages.clear();
    f.write("model.json", &p.to_json());
    let out = f.run(&["check", "model.json"]);
    failure(
        &out,
        1,
        &[
            "model.json",
            "session:",
            "stage reference 0..1 out of range",
        ],
    );
    assert!(!String::from_utf8_lossy(&out.stderr).contains(" | "));
}
