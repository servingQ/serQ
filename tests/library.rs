//! `use "path";`: a program's definitions read from a library file.

mod common;

use std::path::{Path, PathBuf};

use serq::{compile_source, compile_source_at};

/// A fresh directory with these files in it.
fn dir(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let d = std::env::temp_dir().join(format!("serq-library-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for (f, text) in files {
        let p = d.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    d
}

fn compile(d: &Path, main: &str) -> Result<serq::Program, String> {
    let src = std::fs::read_to_string(d.join(main)).unwrap();
    compile_source_at(&common::main_source(&src), Some(d), &common::horizon(10.0))
}

const PROGRAM: &str = "pool kv { cap 100; }
stage engine : step { cost 1; }
workload { arrive batch(1); init { set k = 3; } session { turn; end; } }
server { take(twice(k)); }

";

#[test]
fn a_program_uses_the_definitions_of_a_library() {
    let d = dir(
        "uses",
        &[
            (
                "lib/a.sq",
                "use \"b.sq\";\ndef take(n) { hold kv (cost(kv, n)) { prefill on engine (n) growing kv; } }\n",
            ),
            ("lib/b.sq", "def twice(x) { 2 * x }\n"),
            (
                "main.sq",
                &format!("use \"lib/a.sq\";\nuse \"lib/b.sq\";\n{PROGRAM}"),
            ),
        ],
    );
    // a library `use`s the files next to it, and one read twice is read once
    let used = compile(&d, "main.sq").unwrap();
    let written = compile_source(
        &common::main_source(&PROGRAM.replace(
            "server { take(twice(k)); }",
            "server { hold kv (cost(kv, 2 * k)) { prefill on engine (2 * k) growing kv; } }",
        )),
        &common::horizon(10.0),
    )
    .unwrap();
    assert_eq!(used.to_json(), written.to_json());
}

#[test]
fn an_error_in_a_library_is_shown_in_the_library() {
    let d = dir(
        "error",
        &[
            ("lib.sq", "def take(n) {\n  turn;\n}\n"),
            (
                "main.sq",
                &format!("use \"lib.sq\";\n{PROGRAM}").replace("take(twice(k))", "take(1)"),
            ),
        ],
    );
    let e = compile(&d, "main.sq").unwrap_err();
    assert!(e.contains("lib.sq:2:3:"), "{e}");
    assert!(e.contains("  turn;"), "the library's line: {e}");
    assert!(e.contains("note: in `take`, used at 5:10"), "{e}");
}

#[test]
fn a_library_holds_definitions() {
    let d = dir(
        "only-defs",
        &[
            ("lib.sq", "def f(x) { x }\nlet k = 3;\n"),
            (
                "main.sq",
                "use \"lib.sq\";\nworkload { session { turn; end; \n} }\nserver {\n}\n",
            ),
        ],
    );
    let e = compile(&d, "main.sq").unwrap_err();
    assert!(e.contains("a library holds definitions"), "{e}");
    assert!(e.contains("lib.sq:2:1:"), "{e}");
}

#[test]
fn a_library_definition_is_whole() {
    // the program cannot finish a library's definition
    for (lib, main) in [
        ("def", "take(n) { set a = n; }\n"),
        ("def g(x) { x +", "1 }\n"),
    ] {
        let d = dir(
            "whole",
            &[
                ("lib.sq", lib),
                (
                    "main.sq",
                    &format!(
                        "use \"lib.sq\";\n{main}workload {{ session {{ turn; end; \n}} }}\nserver {{\n}}\n"
                    ),
                ),
            ],
        );
        let e = compile(&d, "main.sq").unwrap_err();
        assert!(
            e.contains("does not end in the file it starts in"),
            "{lib}: {e}"
        );
    }
}

#[test]
fn a_link_note_names_the_library() {
    let d = dir(
        "note",
        &[
            ("lib.sq", "def take(n) {\n  set granted = n;\n}\n"),
            (
                "main.sq",
                "use \"lib.sq\";\nworkload { session { turn; end; \n} }\nserver { take(1); observe x = grantedd;\n}\n",
            ),
        ],
    );
    let e = compile(&d, "main.sq").unwrap_err();
    assert!(e.contains("lib.sq:2:7"), "{e}");
}

#[test]
fn a_library_that_uses_the_program_back_does_not_read_it_again() {
    let d = dir(
        "root",
        &[
            ("lib/a.sq", "use \"../main.sq\";\ndef twice(x) { 2 * x }\n"),
            (
                "main.sq",
                "use \"lib/a.sq\";\nstage svc : fifo;\nworkload { session { turn; end; \n} }\nserver { run svc (cost(svc, twice(1)));\n}\n\n",
            ),
            // two libraries that use each other: the order of their definitions
            // is what is wrong, and the error says so
            ("lib/a2.sq", "use \"b2.sq\";\ndef take(n) { n }\n"),
            ("lib/b2.sq", "use \"a2.sq\";\ndef give(n) { take(n) }\n"),
        ],
    );
    let main = d.join("main.sq");
    std::fs::write(
        &main,
        common::main_source(&std::fs::read_to_string(&main).unwrap()),
    )
    .unwrap();
    serq::load(&main, &common::horizon(10.0)).unwrap();
    let lib = d.join("lib/a2.sq");
    let e = serq::compile_file(
        &common::main_source(&std::fs::read_to_string(&lib).unwrap()),
        &lib,
        &common::horizon(10.0),
    )
    .unwrap_err();
    assert!(
        e.contains("`give` uses `take`, which is defined after it"),
        "{e}"
    );
    let f = serq::frontend::fmt::format_file(
        &common::main_source(&std::fs::read_to_string(&main).unwrap()),
        &main,
    );
    assert!(f.is_ok(), "{f:?}");
}

#[test]
fn blocksize_is_the_pools_block() {
    let src = |pool: &str, e: &str| {
        format!(
            "{pool}\nstage svc : delay;\nworkload {{ session {{ turn; end; \n}} }}\nserver {{ observe b = {e};\n}}\n\n"
        )
    };
    let p = compile_source(
        &common::main_source(&src("pool kv[2] { cap 64; block 16; }", "blocksize(kv[1])")),
        &common::horizon(1.0),
    )
    .unwrap();
    let q = compile_source(
        &common::main_source(&src("pool kv[2] { cap 64; block 16; }", "16")),
        &common::horizon(1.0),
    )
    .unwrap();
    assert_eq!(p.to_json(), q.to_json());
    for (pool, e, want) in [
        ("pool kv { cap 64; }", "blocksize(kv)", "has no `block`"),
        (
            "pool kv { cap 64; block 16; }",
            "blocksize(kz)",
            "unknown pool `kz`",
        ),
        (
            "pool kv { cap 64; block 16; }",
            "blocksize(kv, kv)",
            "takes one pool",
        ),
        (
            "pool kv { cap 64; block 16; }",
            "blocksize(kv + 1)",
            "not an expression",
        ),
        (
            "pool kv[2] { cap 64; block 16; }",
            "blocksize(kv)",
            "is an array",
        ),
        (
            "pool kv[2] { cap 64; block 16; }",
            "blocksize(kv[zzz])",
            "unknown name `zzz`",
        ),
        (
            "pool kv[2] { cap 64; block 16; }",
            "blocksize(kv[~uniform(0, 1)])",
            "index draws",
        ),
    ] {
        let e =
            compile_source(&common::main_source(&src(pool, e)), &common::horizon(1.0)).unwrap_err();
        assert!(e.contains(want), "{e}");
    }
}

#[test]
fn blocksize_is_not_a_constant() {
    let e = compile_source(&common::main_source(
        "pool kv { cap 64; block 16; }\nlet b = blocksize(kv);\nstage svc : delay;\nworkload { session { turn; end; \n} }\nserver {\n}\n"),
        &common::horizon(10.0),
    )
    .unwrap_err();
    assert!(e.contains("`blocksize` is not a constant"), "{e}");
    let e = compile_source(
        &common::main_source(
        "def f(blocksize) { blocksize + 1 }\nstage svc : delay;\nworkload { session { turn; end; \n} }\nserver {\n}\n",
        ),
        &common::horizon(10.0),
    )
    .unwrap_err();
    assert!(e.contains("a word of the language"), "{e}");
}

#[test]
fn a_use_needs_a_file() {
    let e = compile_source(
        &common::main_source(
            "use \"lib.sq\";\nworkload { session { turn; end; \n} }\nserver {\n}\n",
        ),
        &common::horizon(10.0),
    )
    .unwrap_err();
    assert!(e.contains("given as text"), "{e}");
    let d = dir(
        "missing",
        &[(
            "main.sq",
            "use \"nowhere.sq\";\nworkload { session { turn; end; \n} }\nserver {\n}\n",
        )],
    );
    let e = compile(&d, "main.sq").unwrap_err();
    assert!(e.contains("cannot read `nowhere.sq`"), "{e}");
}

#[test]
fn the_library_is_one_definition_of_the_vllm_engine() {
    // the four workload programs read their engine from `lib/vllm.sq`, and
    // none of them writes it out
    for name in ["vllm", "vllm_chat", "vllm_single_turn", "vllm_subagents"] {
        let src = std::fs::read_to_string(serq::program_path(name)).unwrap();
        assert!(src.contains("use \"../../lib/vllm.sq\";"), "{name}");
        assert!(
            src.contains("vllm_request(reqs, kv, engine, prompt, o, t0);"),
            "{name}"
        );
        assert!(
            !src.contains("at admission ("),
            "{name} writes the admission out"
        );
    }
}

#[test]
fn braced_values_and_statements_expand_without_changing_ir() {
    let source = r#"
def twice(x) {
  // A multiline value body with nested parentheses and a conditional.
  x > 0 ? (x * 2) : 0
}
def idle() {}
def take(n) { hold kv (cost(kv, n)) { prefill on engine (n) growing kv; } }
"#;
    let used = compile_source(
        &common::main_source(
            &format!("{source}{PROGRAM}").replace("take(twice(k));", "idle(); take(twice(k));"),
        ),
        &common::horizon(10.0),
    )
    .unwrap();
    let written = compile_source(
        &common::main_source(&PROGRAM.replace(
            "take(twice(k));",
            "hold kv (cost(kv, k > 0 ? (k * 2) : 0)) { prefill on engine (k > 0 ? (k * 2) : 0) growing kv; }",
        )),
        &common::horizon(10.0),
    )
    .unwrap();
    assert_eq!(used.to_json(), written.to_json());
}

#[test]
fn definitions_reject_legacy_syntax_and_mixed_bodies() {
    for (definition, expected) in [
        ("def f(x) = x;", "without a semicolon"),
        (
            "def f(x) { set y = x; y }",
            "cannot end with a result expression",
        ),
        (
            "def f(x) { if x { observe y = x; } x }",
            "cannot end with a result expression",
        ),
        ("def f(x) { x", "is not closed"),
    ] {
        let source = format!(
            "{definition}\nfn main() {{ workload {{ session {{ turn; end; }} }} server {{}} }}"
        );
        let error = compile_source(&source, &common::horizon(10.0)).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
    let source = common::main_source(
        "def f(x) { x; } workload { session { turn; end; } } server { set y = f(1); }",
    );
    let error = compile_source(&source, &common::horizon(10.0)).unwrap_err();
    assert!(error.contains("not an expression"), "{error}");
}
