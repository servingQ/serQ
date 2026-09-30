mod common;

use common::{Fixture, failure};
use serq::frontend::fmt::format;
use serq::{Overrides, compile_source};

#[test]
fn formatting_preserves_program_and_comments_and_is_idempotent() {
    let source = "let B=10;         // token budget\n\
                  let C=10;         // pool capacity\n\
                  // a paragraph\n\n\
                  pool kv { cap C; }\n\
                  stage engine : step { budget B; cost 1; memory kv; }\n\
                  workload { arrive batch(1); session { request; end; } }\n\
                  server {\n\
                  hold kv (1)\n\
                  at admission (hit = 1,\n\
                  value = hit + 1) {\n\
                  prefill (1) growing kv;\n\
                  } cache (1);\n\
                  }\n\
                  run { horizon 1; }\n";
    let formatted = format(source).unwrap();
    assert!(
        formatted.contains(
            "  hold kv (1)\n       at admission (hit = 1,\n                     value = hit + 1) {"
        ),
        "{formatted}"
    );
    assert!(formatted.contains("let B=10;         // token budget"));
    assert!(formatted.contains("// a paragraph\n\n"));
    assert_eq!(format(&formatted).unwrap(), formatted);
    let before = compile_source(source, &Overrides::default())
        .unwrap()
        .to_json();
    let after = compile_source(&formatted, &Overrides::default())
        .unwrap()
        .to_json();
    assert_eq!(after, before);
}

#[test]
fn cli_check_and_write_are_consistent() {
    let fixture = Fixture::new();
    fixture.write(
        "model.serq",
        "stage svc : delay;\nsession {run svc (1); end;}\nrun {horizon 1;}\n",
    );
    failure(
        &fixture.run(&["fmt", "--check", "model.serq"]),
        1,
        &["would reformat model.serq"],
    );
    assert!(fixture.run(&["fmt", "model.serq"]).status.success());
    assert!(
        fixture
            .run(&["fmt", "--check", "model.serq"])
            .status
            .success()
    );
    let once = std::fs::read_to_string(fixture.0.join("model.serq")).unwrap();
    assert!(fixture.run(&["fmt", "model.serq"]).status.success());
    assert_eq!(
        std::fs::read_to_string(fixture.0.join("model.serq")).unwrap(),
        once
    );
}

#[test]
fn invalid_batch_leaves_every_file_untouched() {
    let fixture = Fixture::new();
    let original = "stage svc : delay;\nsession {run svc (1); end;}\nrun {horizon 1;}\n";
    fixture.write("good.serq", original);
    fixture.write("bad.serq", "stage broken : delay; /* open");
    failure(
        &fixture.run(&["fmt", "good.serq", "bad.serq"]),
        1,
        &["bad.serq", "unterminated block comment"],
    );
    assert_eq!(
        std::fs::read_to_string(fixture.0.join("good.serq")).unwrap(),
        original
    );
}
