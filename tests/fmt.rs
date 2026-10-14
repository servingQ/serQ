mod common;

use common::{Fixture, failure};
use serq::frontend::fmt::format;

#[test]
fn formatting_preserves_program_and_comments_and_is_idempotent() {
    let source = "let B=10;         // token budget\n\
        let C=10;         // pool capacity\n\
        // a paragraph\n\n\
        device gpu { kv cap C; }\n\
        engine llm on gpu { tokens cap B; scheduler { advance running; admit waiting while (running.preempted == 0); } worker { forward (1); } }\n\
        pool kv on gpu { }\n\
        server {\n\
          hold kv (cost(kv, 1))\n\
          at admission (hit = 1,\n\
          value = hit + 1) {\n\
            llm (1, 1);\n\
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
    // the workload's file is formatted on its own
    let workload =
        "// one request\nworkload { arrive batch(1);\n\n\n    session { turn; end; } }\n";
    let formatted_workload = format(workload).unwrap();
    assert_eq!(
        formatted_workload,
        "// one request\nworkload { arrive batch(1);\n\n  session { turn; end; } }\n"
    );
    assert_eq!(format(&formatted_workload).unwrap(), formatted_workload);
    let before = common::compile(
        &common::main_source(source),
        workload,
        &common::horizon(1.0),
    )
    .unwrap()
    .to_json();
    let after = common::compile(
        &common::main_source(&formatted),
        &formatted_workload,
        &common::horizon(1.0),
    )
    .unwrap()
    .to_json();
    assert_eq!(after, before);
}

#[test]
fn cli_check_and_write_are_consistent() {
    let fixture = Fixture::new();
    fixture.write(
        "model.sq",
        &common::main_source("stage svc : delay;\nserver {run svc (cost(svc, 1));\n}\n\n"),
    );
    fixture.write("w.sq", "workload { session { turn; end;\n} }\n\n");
    // a deployment and a workload's file are each formatted alone
    for file in ["model.sq", "w.sq"] {
        failure(
            &fixture.run(&["fmt", "--check", file]),
            1,
            &[&format!("would reformat {file}")],
        );
        assert!(fixture.run(&["fmt", file]).status.success());
        assert!(fixture.run(&["fmt", "--check", file]).status.success());
        let once = std::fs::read_to_string(fixture.0.join(file)).unwrap();
        assert!(fixture.run(&["fmt", file]).status.success());
        assert_eq!(std::fs::read_to_string(fixture.0.join(file)).unwrap(), once);
    }
    // and still run together
    let out = fixture.run(&["check", "model.sq", "--workload", "w.sq"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn invalid_batch_leaves_every_file_untouched() {
    let fixture = Fixture::new();
    let original = "stage svc : delay;\nserver {run svc (cost(svc, 1));\n}\n\n";
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
    let source = "def twice(x){\n// value\nx * 2\n}\ndef record(x){observe value = x;}\nfn main(){\nserver { record(twice(3)); }\n}\n";
    let workload = "workload { session { turn; end; } }";
    let formatted = format(source).unwrap();
    assert!(
        formatted.contains("def twice(x) {\n  // value\n  x * 2\n}"),
        "{formatted}"
    );
    assert_eq!(format(&formatted).unwrap(), formatted);
    assert_eq!(
        common::compile(source, workload, &common::horizon(10.0))
            .unwrap()
            .to_json(),
        common::compile(&formatted, workload, &common::horizon(10.0))
            .unwrap()
            .to_json(),
    );
}

/// A run of blank lines is written as one, and none opens or closes a block
/// or the file; a blank line inside a block comment is the comment's.
#[test]
fn blank_lines_are_one_and_none_at_the_edge_of_a_block() {
    // a comment after `{` or before `}` does not keep the blank line by it
    let source = "\n\nlet C = 1;\n\n\n/* a\n\n\nb */\nfn main() { // c\n\n\
        stage svc : delay;\n\n\n\
        server { run svc (cost(svc, C)); }\n  // last\n\n}\n\n\n";
    let formatted = format(source).unwrap();
    assert_eq!(
        formatted,
        "let C = 1;\n\n/* a\n\n\nb */\nfn main() { // c\n\
         \x20 stage svc : delay;\n\n\
         \x20 server { run svc (cost(svc, C)); }\n\
         \x20 // last\n}\n"
    );
    assert_eq!(format(&formatted).unwrap(), formatted);
}
