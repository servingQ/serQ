mod common;
use common::{Fixture, PROGRAM, failure};
use serq::frontend::parser;
use serq::{Overrides, compile_source};

#[test]
fn unknown_references_show_the_use_and_a_declaration_of_the_right_kind() {
    let f = Fixture::new();
    f.write(
        "model.sq",
        &PROGRAM.replace("run svc (rate)", "run svcc (rate)"),
    );
    failure(
        &f.run(&["check", "model.sq"]),
        1,
        &[
            "model.sq: 4:15:",
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
    f.write("model.sq", src);
    failure(
        &f.run(&["check", "model.sq"]),
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
    let src = "pool kv { cap 10; }\nworkload { arrive batch(1); session { request; end; } }\nserver {\n  hold kv (amount) at admission (amount = missing) { }\n}\nrun { horizon 10; }";
    let err = compile_source(src, &Overrides::default()).unwrap_err();
    // `missing`, in the binding, is where the failed expression was written.
    assert!(err.contains("4:43:"), "{err}");
    assert!(err.contains("unknown name `missing`"), "{err}");
}

#[test]
fn queue_expansion_keeps_argument_and_stage_declaration_locations() {
    let src = "queue engine : prefill {\n  serve fifo;\n  prefill (prompt) { run (prompt); }\n}\nqueue gw : gateway { route {\n  engine.prefill (missing);\n} }\nworkload { arrive batch(1); session { request gw; end; } }\nrun { horizon 10; }";
    let err = compile_source(src, &Overrides::default()).unwrap_err();
    // Parameter substitution must point to the argument at the call site,
    // rather than the parameter inside the queue's entry.
    assert!(err.contains("6:19:"), "{err}");
    assert!(err.contains("unknown name `missing`"), "{err}");
    assert!(err.contains("6 |   engine.prefill (missing);"), "{err}");

    let src = "queue engine : prefill {\n  pool kv { cap 10; admit via engin; }\n  serve step { cost 1; memory kv; }\n  prefill (prompt) { hold kv (prompt) { prefill (prompt) growing kv; } }\n}\nrun { horizon 10; }";
    let err = compile_source(src, &Overrides::default()).unwrap_err();
    assert!(err.contains("2:31:"), "{err}");
    assert!(err.contains("unknown stage `engin`"), "{err}");
    assert!(err.contains("did you mean stage `engine`?"), "{err}");
    assert!(err.contains("declared at 1:7"), "{err}");
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

#[test]
fn ownership_checks_ignore_locations_but_keep_index_syntax() {
    // The same target is written on three different lines. Comparing source
    // spans would reject it; folding indices would incorrectly accept j too.
    let index = "i == 0 ? min(i, 1) : -i";
    let program = |target: &str| {
        format!(
            "pool q[2] {{ cap 10; }}
         session {{
           set i = 0; set j = 0;
           hold q[{index}] (1) {{
             load q[{target}] (1);
           }} lease q[{target}] (1);
           release q[{target}];
           end;
         }}
         run {{ horizon 10; }}"
        )
    };
    compile_source(&program(index), &Overrides::default()).expect("same written target");
    let err = compile_source(&program("j == 0 ? min(j, 1) : -j"), &Overrides::default())
        .expect_err("different written target, even though i and j are both zero");
    assert!(err.contains("index included"), "{err}");
}

#[test]
fn a_constant_that_is_nan_is_refused_and_an_infinity_is_not() {
    let src = "let x = 1;\npool kv { cap 100; block 16; }\nstage s : fifo(1);\n\
               workload { arrive poisson(1); }\nsession { run s (x); end; }\n\
               run { horizon 10; seed 1; }";
    let refused = |src: &str, ov: &Overrides, what: &str| {
        let err = compile_source(src, ov).expect_err(what);
        assert!(
            err.contains(&format!("{what} is NaN")) && err.contains("help:"),
            "{err}"
        );
    };
    for e in ["0/0", "inf - inf"] {
        let none = Overrides::default();
        refused(
            &src.replace("let x = 1", &format!("let x = {e}")),
            &none,
            "`let x`",
        );
        refused(
            &src.replace("cap 100", &format!("cap {e}")),
            &none,
            "pool `kv`: cap",
        );
        refused(
            &src.replace("block 16", &format!("block {e}")),
            &none,
            "pool `kv`: block",
        );
        refused(
            &src.replace("fifo(1)", &format!("fifo({e})")),
            &none,
            "stage `s`: fifo server count",
        );
        refused(
            &src.replace("poisson(1)", &format!("poisson({e})")),
            &none,
            "the poisson rate",
        );
        refused(
            &src.replace("seed 1", &format!("seed {e}")),
            &none,
            "the seed",
        );
        refused(
            &src.replace("horizon 10", &format!("horizon {e}")),
            &none,
            "the horizon",
        );
        let mut ov = Overrides::default();
        ov.set("x", e).unwrap();
        refused(src, &ov, "--set x: the value");
    }
    let mut ov = Overrides::default();
    ov.set_num("x", f64::NAN).unwrap();
    refused(src, &ov, "--set x: the value");
    let mut ov = Overrides::default();
    ov.set_num("x", f64::INFINITY).unwrap();
    compile_source(src, &ov).expect("an infinity is `inf`");
    compile_source(&src.replace("cap 100", "cap inf"), &Overrides::default())
        .expect("an infinite cap");
    // a constant with a place (a call) keeps it in the error
    let err = compile_source(
        &src.replace("cap 100", "cap sqrt(-1)"),
        &Overrides::default(),
    )
    .expect_err("a NaN call");
    assert!(err.contains("2:15:") && err.contains("cap is NaN"), "{err}");
}

#[test]
fn a_def_given_from_outside_is_the_program_written_with_that_body() {
    let src = |service: &str, key: &str| {
        format!(
            "let lam = 0.5;\ndef service() = {service};\ndef key(x) = {key};\n\
             def twice(x) {{ set y = x * 2; }}\nstage svc : fifo;\n\
             workload {{ arrive poisson(lam); }}\n\
             session {{ set c = 1; run svc (service()); observe k = key(c); end; }}\n\
             run {{ horizon 100; seed 3; }}"
        )
    };
    let base = src("~exp(1)", "x");
    let mut ov = Overrides::default();
    ov.define("service", "c == 1 ? ~erlang(4, 1) : ~det(1)")
        .unwrap();
    ov.define("key", "x + 1").unwrap();
    let given = compile_source(&base, &ov).unwrap();
    let written = compile_source(
        &src("c == 1 ? ~erlang(4, 1) : ~det(1)", "x + 1"),
        &Overrides::default(),
    )
    .unwrap();
    assert_eq!(given.to_json(), written.to_json());
    assert_ne!(
        given.to_json(),
        compile_source(&base, &Overrides::default())
            .unwrap()
            .to_json()
    );

    let refused = |name: &str, body: &str, want: &str| {
        let mut ov = Overrides::default();
        let err = match ov.define(name, body) {
            Err(e) => e,
            Ok(()) => compile_source(&base, &ov).expect_err(want),
        };
        assert!(err.contains(want), "{err}");
    };
    refused("nope", "1", "unknown --def `nope`");
    refused("twice", "1", "`def twice` is statements");
    refused("service", "~exp(", "invalid expression in --def");
    refused("service", "zz", "unknown name `zz`");
    refused("not a name", "1", "");
}
