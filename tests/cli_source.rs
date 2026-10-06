mod common;
use common::{Fixture, PROGRAM, failure};
use serq::frontend::parser;
use serq::{Overrides, compile_source};

#[test]
fn unknown_references_show_the_use_and_a_declaration_of_the_right_kind() {
    let f = Fixture::new();
    f.write(
        "model.sq",
        &common::main_source(
            &PROGRAM.replace("run svc (cost(svc, rate))", "run svcc (cost(svcc, rate))"),
        ),
    );
    failure(
        &f.run(&["check", "model.sq"]),
        1,
        &[
            "model.sq: 4:14:",
            "unknown stage `svcc`",
            "4 | server { run svcc (cost(svcc, rate)); }",
            "^^^^",
            "did you mean stage `svc`?",
            "declared at 2:7",
        ],
    );
    // The first spelling is a valid stage, but the second is an unknown pool.
    // Searching the token stream for the first matching name would misdiagnose it.
    let src = "stage kvv : fifo;\npool kv { cap 10; }\nworkload { session { request; hold kvv (cost(kvv, 1)) { end; } \n} }\nserver {\n}\n";
    f.write("model.sq", &common::main_source(src));
    failure(
        &f.run(&["check", "model.sq"]),
        1,
        &[
            "3:36:",
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
            "workload { session { request; \n} }\nserver { set x = min(typo, 1);\n}\n",
            "3:22:",
            "unknown name `typo`",
        ),
        (
            "pool kv { admit via engin; }\nstage engine : step { cost 1; }\n",
            "1:33:",
            "unknown stage `engin`",
        ),
        (
            "pool kv { cap 10; }\nstage engine : step { memory kvv; cost 1; }\n",
            "2:30:",
            "unknown pool `kvv`",
        ),
        (
            "// 한글 주석\nlet 용량 = 10;\nworkload { session { request; \n} }\nserver { set x = 용랑;\n}\n",
            "5:18:",
            "unknown name `용랑`",
        ),
        (
            "use \"std/args\"; let rate = args.number(\"rate\", 1);\nlet x = min(raet, 1);\n",
            "2:13:",
            "`raet` is not a constant",
        ),
    ] {
        let err = compile_source(&common::main_source(src), &common::horizon(1.0)).unwrap_err();
        assert!(err.contains(location), "{err}");
        assert!(err.contains(cause), "{err}");
        assert!(err.contains("help:"), "{err}");
    }
}

#[test]
fn duplicate_declarations_point_to_both_sites() {
    for (src, first, second) in [
        ("stage svc : fifo;\nstage svc : delay;\n", "1:19", "2:7:"),
        ("pool kv { cap 1; }\npool kv { cap 2; }\n", "1:18", "2:6:"),
    ] {
        let err = compile_source(&common::main_source(src), &common::horizon(1.0)).unwrap_err();
        assert!(err.contains(second), "{err}");
        assert!(err.contains(&format!("first declared at {first}")), "{err}");
    }
}

#[test]
fn desugaring_keeps_server_and_header_binding_locations() {
    let src = "stage svc : fifo;\nworkload { arrive batch(1); session { request; end; } }\nserver {\n  run svcc (cost(svcc, 1));\n}\n";
    let err = compile_source(&common::main_source(src), &common::horizon(10.0)).unwrap_err();
    assert!(err.contains("4:7:"), "{err}");
    assert!(err.contains("4 |   run svcc (cost(svcc, 1));"), "{err}");
    let src = "pool kv { cap 10; }\nworkload { arrive batch(1); session { request; end; } }\nserver {\n  hold kv (cost(kv, amount)) at admission (amount = missing) { }\n}\n";
    let err = compile_source(&common::main_source(src), &common::horizon(10.0)).unwrap_err();
    // `missing`, in the binding, is where the failed expression was written.
    assert!(err.contains("4:53:"), "{err}");
    assert!(err.contains("unknown name `missing`"), "{err}");
}

#[test]
fn queue_expansion_keeps_argument_and_stage_declaration_locations() {
    let src = "queue engine : prefill {\n  serve fifo;\n  prefill (prompt) { run (cost(engine, prompt)); }\n}\nqueue gw : gateway { route {\n  engine.prefill (missing);\n} }\nworkload { arrive batch(1); session { request gw; end; } }\n";
    let err = compile_source(&common::main_source(src), &common::horizon(10.0)).unwrap_err();
    // Parameter substitution must point to the argument at the call site,
    // rather than the parameter inside the queue's entry.
    assert!(err.contains("6:19:"), "{err}");
    assert!(err.contains("unknown name `missing`"), "{err}");
    assert!(err.contains("6 |   engine.prefill (missing);"), "{err}");

    let src = "queue engine : prefill {\n  pool kv { cap 10; admit via engin; }\n  serve step { cost 1; memory kv; }\n  prefill (prompt) { hold kv (cost(kv, prompt)) { prefill (prompt) growing kv; } }\n}\n";
    let err = compile_source(&common::main_source(src), &common::horizon(10.0)).unwrap_err();
    assert!(err.contains("2:31:"), "{err}");
    assert!(err.contains("unknown stage `engin`"), "{err}");
    assert!(err.contains("did you mean stage `engine`?"), "{err}");
    assert!(err.contains("declared at 1:19"), "{err}");
}

#[test]
fn ambiguous_suggestions_and_override_spans_are_not_misleading() {
    let src = "stage cat : fifo; stage cut : fifo; workload { session { request; \n} }\nserver { run cot (cost(cot, 1));\n} ";
    let err = compile_source(&common::main_source(src), &common::horizon(1.0)).unwrap_err();
    assert!(!err.contains("did you mean"), "{err}");
    let ov = Overrides {
        lets: vec![("rate".into(), parser::parse_expr("missing").unwrap())],
        ..common::horizon(1.0)
    };
    let err = compile_source(&common::main_source(PROGRAM), &ov).unwrap_err();
    assert!(err.contains("the program argument `rate`"), "{err}");
    assert!(
        !err.contains("1 |"),
        "an override is not line 1 of the program: {err}"
    );
}

#[test]
fn eof_errors_keep_the_eof_location_and_never_panic() {
    let err = compile_source("fn main() { stage svc : fifo;", &common::horizon(10.0)).unwrap_err();
    assert!(err.contains("1:30:"), "{err}");
    let err = parser::parse_expr("").unwrap_err();
    assert_eq!((err.line, err.col), (1, 1));
    let err = compile_source("fn main() { stage svc[", &common::horizon(10.0)).unwrap_err();
    assert!(err.contains("1:23:"), "{err}");
}

#[test]
fn ir_errors_use_ir_context_instead_of_a_fabricated_source_location() {
    let f = Fixture::new();
    let mut p = compile_source(&common::main_source(PROGRAM), &common::horizon(10.0)).unwrap();
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
         workload {{ session {{ request;
           end;

}} }}
server {{
           set i = 0; set j = 0;
           hold q[{index}] (cost(q, 1)) {{
             load q[{target}] (cost(q, 1));
           }} lease q[{target}] (1);
           release q[{target}];
}}
         "
        )
    };
    compile_source(
        &common::main_source(&program(index)),
        &common::horizon(10.0),
    )
    .expect("same written target");
    let err = compile_source(
        &common::main_source(&program("j == 0 ? min(j, 1) : -j")),
        &common::horizon(10.0),
    )
    .expect_err("different written target, even though i and j are both zero");
    assert!(err.contains("index included"), "{err}");
}

#[test]
fn a_constant_that_is_nan_is_refused_and_an_infinity_is_not() {
    let src = "use \"std/args\"; let x = args.number(\"x\", 1);
pool kv { cap 100; block 16; }
stage s : fifo(1);
workload { arrive poisson(1);
  session { request; end;
  }
}
server { run s (cost(s, x));
}
";
    let refused = |src: &str, ov: &Overrides, what: &str| {
        let err = compile_source(&common::main_source(src), ov).expect_err(what);
        assert!(
            err.contains(&format!("{what} is NaN")) && err.contains("help:"),
            "{err}"
        );
    };
    for e in ["0/0", "inf - inf"] {
        let none = Overrides {
            seed: Some(1),
            ..common::horizon(10.0)
        };
        refused(
            &src.replace("args.number(\"x\", 1)", &format!("args.number(\"x\", {e})")),
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
        let mut ov = Overrides {
            seed: Some(1),
            ..common::horizon(10.0)
        };
        ov.set("x", e).unwrap();
        refused(src, &ov, "the program argument `x`: the value");
    }
    let mut ov = Overrides {
        seed: Some(1),
        ..common::horizon(10.0)
    };
    ov.set_num("x", f64::NAN).unwrap();
    refused(src, &ov, "the program argument `x`: the value");
    let mut ov = Overrides {
        seed: Some(1),
        ..common::horizon(10.0)
    };
    ov.set_num("x", f64::INFINITY).unwrap();
    compile_source(&common::main_source(src), &ov).expect("an infinity is `inf`");
    compile_source(
        &common::main_source(&src.replace("cap 100", "cap inf")),
        &Overrides {
            seed: Some(1),
            ..common::horizon(10.0)
        },
    )
    .expect("an infinite cap");
    for horizon in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
        let e = compile_source(&common::main_source(src), &common::horizon(horizon)).unwrap_err();
        assert!(e.contains("horizon"), "{e}");
    }
    // a constant with a place (a call) keeps it in the error
    let err = compile_source(
        &common::main_source(&src.replace("cap 100", "cap sqrt(-1)")),
        &Overrides {
            seed: Some(1),
            ..common::horizon(10.0)
        },
    )
    .expect_err("a NaN call");
    assert!(err.contains("2:15:") && err.contains("cap is NaN"), "{err}");
}

#[test]
fn a_def_given_from_outside_is_the_program_written_with_that_body() {
    let src = |service: &str, key: &str| {
        format!(
            "let lam = 0.5;
def service() {{ {service} }}
def key(x) {{ {key} }}
def twice(x) {{ set y = x * 2; }}
stage svc : fifo;
workload {{ arrive poisson(lam);
  session {{ request; end;
  }}
}}
server {{ set c = 1; run svc (cost(svc, service())); observe k = key(c);
}}
"
        )
    };
    let base = src("~exp(1)", "x");
    let mut ov = Overrides {
        seed: Some(3),
        ..common::horizon(100.0)
    };
    ov.define("service", "c == 1 ? ~erlang(4, 1) : ~det(1)")
        .unwrap();
    ov.define("key", "x + 1").unwrap();
    let given = compile_source(&common::main_source(&base), &ov).unwrap();
    let written = compile_source(
        &common::main_source(&src("c == 1 ? ~erlang(4, 1) : ~det(1)", "x + 1")),
        &Overrides {
            seed: Some(3),
            ..common::horizon(100.0)
        },
    )
    .unwrap();
    assert_eq!(given.to_json(), written.to_json());
    assert_ne!(
        given.to_json(),
        compile_source(
            &common::main_source(&base),
            &Overrides {
                seed: Some(3),
                ..common::horizon(100.0)
            }
        )
        .unwrap()
        .to_json()
    );

    let refused = |name: &str, body: &str, want: &str| {
        let mut ov = Overrides {
            seed: Some(3),
            ..common::horizon(100.0)
        };
        let err = match ov.define(name, body) {
            Err(e) => e,
            Ok(()) => compile_source(&common::main_source(&base), &ov).expect_err(want),
        };
        assert!(err.contains(want), "{err}");
    };
    refused("nope", "1", "unknown `def` override `nope`");
    refused("twice", "1", "`def twice` is statements");
    refused(
        "service",
        "~exp(",
        "invalid expression in the `def` override",
    );
    refused("service", "zz", "unknown name `zz`");
    refused("not a name", "1", "");
}
