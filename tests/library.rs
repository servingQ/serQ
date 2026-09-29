//! `use "path";`: a program's definitions read from a library file.

use std::path::{Path, PathBuf};

use seq::{Overrides, compile_source, compile_source_at};

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

fn compile(d: &Path, main: &str) -> Result<seq::Program, String> {
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
                "lib/a.seq",
                "use \"b.seq\";\ndef take(n) { admit if kv (n) fit { prefill on engine (n) growing kv; } }\n",
            ),
            ("lib/b.seq", "def twice(x) = 2 * x;\n"),
            (
                "main.seq",
                &format!("use \"lib/a.seq\";\nuse \"lib/b.seq\";\n{PROGRAM}"),
            ),
        ],
    );
    // a library `use`s the files next to it, and one read twice is read once
    let used = compile(&d, "main.seq").unwrap();
    let written = compile_source(
        &PROGRAM.replace(
            "server { take(twice(k)); }",
            "server { admit if kv (2 * k) fit { prefill on engine (2 * k) growing kv; } }",
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
            ("lib.seq", "def take(n) {\n  enter kv (n) { }\n}\n"),
            (
                "main.seq",
                &format!("use \"lib.seq\";\n{PROGRAM}").replace("take(twice(k))", "take(1)"),
            ),
        ],
    );
    let e = compile(&d, "main.seq").unwrap_err();
    assert!(e.contains("lib.seq:2:3:"), "{e}");
    assert!(e.contains("  enter kv (n) { }"), "the library's line: {e}");
    assert!(e.contains("note: in `take`, used at 5:10"), "{e}");
}

#[test]
fn a_library_holds_definitions() {
    let d = dir(
        "only-defs",
        &[
            ("lib.seq", "def f(x) = x;\nlet k = 3;\n"),
            ("main.seq", "use \"lib.seq\";\nsession { end; }\n"),
        ],
    );
    let e = compile(&d, "main.seq").unwrap_err();
    assert!(e.contains("a library holds definitions"), "{e}");
    assert!(e.contains("lib.seq:2:1:"), "{e}");
}

#[test]
fn a_use_needs_a_file() {
    let e = compile_source(
        "use \"lib.seq\";\nsession { end; }\n",
        &Overrides::default(),
    )
    .unwrap_err();
    assert!(e.contains("given as text"), "{e}");
    let d = dir(
        "missing",
        &[("main.seq", "use \"nowhere.seq\";\nsession { end; }\n")],
    );
    let e = compile(&d, "main.seq").unwrap_err();
    assert!(e.contains("cannot read `nowhere.seq`"), "{e}");
}

#[test]
fn the_library_is_one_definition_of_the_vllm_engine() {
    // the four workload programs and the P/D program read their engine from
    // `lib/vllm.seq`, and none of them writes it out
    for name in ["vllm", "vllm_chat", "vllm_single_turn", "vllm_subagents"] {
        let src = std::fs::read_to_string(seq::program_path(name)).unwrap();
        assert!(src.contains("use \"../../lib/vllm.seq\";"), "{name}");
        assert!(
            src.contains("vllm_request(reqs, kv, engine, prompt, o, bs, t0);"),
            "{name}"
        );
        assert!(!src.contains("admit if"), "{name} writes the admission out");
    }
}
