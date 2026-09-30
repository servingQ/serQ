//! `use "path";`: a program's definitions read from a library file.

use std::path::{Path, PathBuf};

use serq::{Overrides, compile_source, compile_source_at};

/// A fresh directory with these files in it.
fn dir(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let d = std::env::temp_dir().join(format!("seq-library-{name}-{}", std::process::id()));
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
    compile_source_at(&src, Some(d), &Overrides::default())
}

const PROGRAM: &str = "pool kv { cap 100; }
stage engine : step { cost 1; }
workload { arrive batch(1); init { set k = 3; } session { request; end; } }
server { take(twice(k)); }
run { horizon 10; }
";

#[test]
fn a_program_uses_the_definitions_of_a_library() {
    let d = dir(
        "uses",
        &[
            (
                "lib/a.serq",
                "use \"b.serq\";\ndef take(n) { hold kv (n) { prefill on engine (n) growing kv; } }\n",
            ),
            ("lib/b.serq", "def twice(x) = 2 * x;\n"),
            (
                "main.serq",
                &format!("use \"lib/a.serq\";\nuse \"lib/b.serq\";\n{PROGRAM}"),
            ),
        ],
    );
    // a library `use`s the files next to it, and one read twice is read once
    let used = compile(&d, "main.serq").unwrap();
    let written = compile_source(
        &PROGRAM.replace(
            "server { take(twice(k)); }",
            "server { hold kv (2 * k) { prefill on engine (2 * k) growing kv; } }",
        ),
        &Overrides::default(),
    )
    .unwrap();
    assert_eq!(used.to_json(), written.to_json());
}

#[test]
fn an_error_in_a_library_is_shown_in_the_library() {
    let d = dir(
        "error",
        &[
            ("lib.serq", "def take(n) {\n  turn;\n}\n"),
            (
                "main.serq",
                &format!("use \"lib.serq\";\n{PROGRAM}").replace("take(twice(k))", "take(1)"),
            ),
        ],
    );
    let e = compile(&d, "main.serq").unwrap_err();
    assert!(e.contains("lib.serq:2:3:"), "{e}");
    assert!(e.contains("  turn;"), "the library's line: {e}");
    assert!(e.contains("note: in `take`, used at 5:10"), "{e}");
}

#[test]
fn a_library_holds_definitions() {
    let d = dir(
        "only-defs",
        &[
            ("lib.serq", "def f(x) = x;\nlet k = 3;\n"),
            ("main.serq", "use \"lib.serq\";\nsession { end; }\n"),
        ],
    );
    let e = compile(&d, "main.serq").unwrap_err();
    assert!(e.contains("a library holds definitions"), "{e}");
    assert!(e.contains("lib.serq:2:1:"), "{e}");
}

#[test]
fn a_library_definition_is_whole() {
    // the program cannot finish a library's definition
    for (lib, main) in [
        ("def", "take(n) { set a = n; }\n"),
        ("def g(x) = x +", "1;\n"),
    ] {
        let d = dir(
            "whole",
            &[
                ("lib.serq", lib),
                (
                    "main.serq",
                    &format!("use \"lib.serq\";\n{main}session {{ end; }}\n"),
                ),
            ],
        );
        let e = compile(&d, "main.serq").unwrap_err();
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
            ("lib.serq", "def take(n) {\n  set admitted = n;\n}\n"),
            (
                "main.serq",
                "use \"lib.serq\";\nsession { take(1); observe x = admittedd; end; }\n",
            ),
        ],
    );
    let e = compile(&d, "main.serq").unwrap_err();
    assert!(e.contains("lib.serq:2:7"), "{e}");
}

#[test]
fn a_library_that_uses_the_program_back_does_not_read_it_again() {
    let d = dir(
        "root",
        &[
            (
                "lib/a.serq",
                "use \"../main.serq\";\ndef twice(x) = 2 * x;\n",
            ),
            (
                "main.serq",
                "use \"lib/a.serq\";\nstage svc : fifo;\nsession { run svc (twice(1)); end; }\nrun { horizon 10; }\n",
            ),
            // two libraries that use each other: the order of their definitions
            // is what is wrong, and the error says so
            ("lib/a2.serq", "use \"b2.serq\";\ndef take(n) = n;\n"),
            ("lib/b2.serq", "use \"a2.serq\";\ndef give(n) = take(n);\n"),
        ],
    );
    let main = d.join("main.serq");
    serq::load(&main, &Overrides::default()).unwrap();
    let lib = d.join("lib/a2.serq");
    let e = serq::compile_file(
        &std::fs::read_to_string(&lib).unwrap(),
        &lib,
        &Overrides::default(),
    )
    .unwrap_err();
    assert!(
        e.contains("`give` uses `take`, which is defined after it"),
        "{e}"
    );
    let f = serq::frontend::fmt::format_file(&std::fs::read_to_string(&main).unwrap(), &main);
    assert!(f.is_ok(), "{f:?}");
}

#[test]
fn blocksize_is_the_pools_block() {
    let src = |pool: &str, e: &str| {
        format!(
            "{pool}\nstage svc : delay;\nsession {{ observe b = {e}; end; }}\nrun {{ horizon 1; }}\n"
        )
    };
    let p = compile_source(
        &src("pool kv[2] { cap 64; block 16; }", "blocksize(kv[1])"),
        &Overrides::default(),
    )
    .unwrap();
    let q = compile_source(
        &src("pool kv[2] { cap 64; block 16; }", "16"),
        &Overrides::default(),
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
        let e = compile_source(&src(pool, e), &Overrides::default()).unwrap_err();
        assert!(e.contains(want), "{e}");
    }
}

#[test]
fn blocksize_is_not_a_constant() {
    let e = compile_source(
        "pool kv { cap 64; block 16; }\nlet b = blocksize(kv);\nstage svc : delay;\nsession { end; }\n",
        &Overrides::default(),
    )
    .unwrap_err();
    assert!(e.contains("`blocksize` is not a constant"), "{e}");
    let e = compile_source(
        "def f(blocksize) = blocksize + 1;\nstage svc : delay;\nsession { end; }\n",
        &Overrides::default(),
    )
    .unwrap_err();
    assert!(e.contains("a word of the language"), "{e}");
}

#[test]
fn a_use_needs_a_file() {
    let e = compile_source(
        "use \"lib.serq\";\nsession { end; }\n",
        &Overrides::default(),
    )
    .unwrap_err();
    assert!(e.contains("given as text"), "{e}");
    let d = dir(
        "missing",
        &[("main.serq", "use \"nowhere.serq\";\nsession { end; }\n")],
    );
    let e = compile(&d, "main.serq").unwrap_err();
    assert!(e.contains("cannot read `nowhere.serq`"), "{e}");
}

#[test]
fn the_library_is_one_definition_of_the_vllm_engine() {
    // the four workload programs read their engine from `lib/vllm.serq`, and
    // none of them writes it out
    for name in ["vllm", "vllm_chat", "vllm_single_turn", "vllm_subagents"] {
        let src = std::fs::read_to_string(serq::program_path(name)).unwrap();
        assert!(src.contains("use \"../../lib/vllm.serq\";"), "{name}");
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
