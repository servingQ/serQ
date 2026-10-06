mod common;

use common::{Fixture, failure};
use serq::compile_source;
use serq::frontend::fmt::format;

#[test]
fn formatting_preserves_program_and_comments_and_is_idempotent() {
    let source = "let B=10;         // token budget\n\
        let C=10;         // pool capacity\n\
        // a paragraph\n\n\
        pool kv { cap C; }\n\
        stage engine : step { budget B; cost 1; memory kv; }\n\
        workload { arrive batch(1); session { turn; end; } }\n\
        server {\n\
          hold kv (cost(kv, 1))\n\
          at admission (hit = 1,\n\
          value = hit + 1) {\n\
            run engine prefill (cost(engine, 1)) growing kv;\n\
          } cache (cost(kv, 1));\n\
        }\n\
        \n";
    let formatted = format(&common::main_source(source)).unwrap();
    assert!(
        formatted.contains(
            "    hold kv (cost(kv, 1))\n         at admission (hit = 1,\n                       value = hit + 1) {"
        ),
        "{formatted}"
    );
    assert!(formatted.contains("let B=10;         // token budget"));
    assert!(formatted.contains("// a paragraph\n\n"));
    assert_eq!(format(&common::main_source(&formatted)).unwrap(), formatted);
    let before = compile_source(&common::main_source(source), &common::horizon(1.0))
        .unwrap()
        .to_json();
    let after = compile_source(&common::main_source(&formatted), &common::horizon(1.0))
        .unwrap()
        .to_json();
    assert_eq!(after, before);
}

#[test]
fn cli_check_and_write_are_consistent() {
    let fixture = Fixture::new();
    fixture.write(
        "model.sq",
        &common::main_source(
            "stage svc : delay;\nworkload { session { turn; end;\n} }\nserver {run svc (cost(svc, 1));\n}\n\n",
        ),
    );
    failure(
        &fixture.run(&["fmt", "--check", "model.sq"]),
        1,
        &["would reformat model.sq"],
    );
    assert!(fixture.run(&["fmt", "model.sq"]).status.success());
    assert!(
        fixture
            .run(&["fmt", "--check", "model.sq"])
            .status
            .success()
    );
    let once = std::fs::read_to_string(fixture.0.join("model.sq")).unwrap();
    assert!(fixture.run(&["fmt", "model.sq"]).status.success());
    assert_eq!(
        std::fs::read_to_string(fixture.0.join("model.sq")).unwrap(),
        once
    );
}

#[test]
fn invalid_batch_leaves_every_file_untouched() {
    let fixture = Fixture::new();
    let original = "stage svc : delay;\nworkload { session { turn; end;\n} }\nserver {run svc (cost(svc, 1));\n}\n\n";
    fixture.write("good.sq", &common::main_source(original));
    fixture.write(
        "bad.sq",
        &common::main_source("stage broken : delay; /* open"),
    );
    failure(
        &fixture.run(&["fmt", "good.sq", "bad.sq"]),
        1,
        &["bad.sq", "unterminated block comment"],
    );
    assert_eq!(
        std::fs::read_to_string(fixture.0.join("good.sq")).unwrap(),
        common::main_source(original)
    );
}

#[test]
fn braced_definitions_format_and_preserve_expansion() {
    let source = "def twice(x){\n// value\nx * 2\n}\ndef record(x){observe value = x;}\nfn main(){\nworkload { session { turn; end; } }\nserver { record(twice(3)); }\n}\n";
    let formatted = format(source).unwrap();
    assert!(
        formatted.contains("def twice(x) {\n  // value\n  x * 2\n}"),
        "{formatted}"
    );
    assert_eq!(format(&formatted).unwrap(), formatted);
    assert_eq!(
        compile_source(source, &common::horizon(10.0))
            .unwrap()
            .to_json(),
        compile_source(&formatted, &common::horizon(10.0))
            .unwrap()
            .to_json(),
    );
}
