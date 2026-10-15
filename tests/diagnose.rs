//! `serq::diagnose`, what an editor reads of a refusal
//! (`docs/reference/diagnostics.md`): the message `serq check` prints, its
//! code, its file and place, and the code's fix-it as edits, which made
//! take the refusal away. Each case is an example with one thing changed.

use std::path::Path;

use serq::frontend::diagnostic::{Code, Diagnostic, Fix};
use serq::{Overrides, diagnose};

/// An example's deployment and the workload file it is paired with.
fn pair(path: &str) -> (String, String) {
    let w = serq::paired_workload(Path::new(path)).expect("a pair");
    (
        std::fs::read_to_string(path).unwrap(),
        std::fs::read_to_string(w).unwrap(),
    )
}

const PULL: &str = "examples/pd-disaggregation/llmd_nixl_pull.sq";
const PUSH: &str = "examples/pd-disaggregation/vllm_nixl_push.sq";

fn with(src: &str, from: &str, to: &str) -> String {
    assert!(src.contains(from), "no {from:?}");
    src.replacen(from, to, 1)
}

fn refused(d: &str, w: &str, code: Code) -> Diagnostic {
    let r = diagnose(d, Some(w), &Overrides::default()).expect("refused");
    assert_eq!(r.code, Some(code.name()), "{}", r.message);
    assert_eq!(r.file, Some("deployment"), "{}", r.message);
    r
}

fn fixed(src: &str, fix: &Fix) -> String {
    let mut out = src.to_string();
    let mut edits = fix.edits.clone();
    edits.sort_by_key(|e| std::cmp::Reverse(e.range.0));
    for e in edits {
        out.replace_range(e.range.0..e.range.1, &e.text);
    }
    out
}

#[test]
fn an_example_has_none() {
    for path in [PULL, PUSH] {
        let (d, w) = pair(path);
        let r = diagnose(&d, Some(&w), &Overrides::default());
        assert!(r.is_none(), "{path}: {r:?}");
    }
}

/// The message is `serq check`'s, the place is the token the caret is under.
#[test]
fn a_reserved_word_has_its_code_and_place() {
    let (d, w) = pair(PULL);
    let d = with(
        &d,
        "let NP = 2;",
        "def twice(uniform) { uniform * 2 }\nlet NP = 2;",
    );
    let r = refused(&d, &w, Code::ReservedWord);
    let check =
        serq::compile_paired_inspected(&d, &w, ("deployment", "workload"), &Overrides::default());
    assert_eq!(check.unwrap_err(), r.message);
    let range = r.range.unwrap();
    assert_eq!(&d[range.0..range.1], "uniform");
    assert!(r.fixes.is_empty());
    assert!(r.to_json().contains("\"code\":\"E-reserved-word\""));
}

/// A pull whose source is called in a `fork` that holds the call alone:
/// the fork taken away, it compiles.
#[test]
fn a_forked_pull_is_unforked() {
    let (d, w) = pair(PULL);
    let d = with(
        &d,
        "        P[i].prefill (input);",
        "        fork { P[i].prefill (input); }",
    );
    let r = refused(&d, &w, Code::PullForked);
    assert_eq!(r.fixes.len(), 1, "{}", r.message);
    let after = fixed(&d, &r.fixes[0]);
    assert!(after.contains("        P[i].prefill (input);"), "{after}");
    let next = diagnose(&after, Some(&w), &Overrides::default());
    assert!(next.is_none(), "{next:?}");
}

/// A fork with more than the call in it is the program's to divide: no fix.
#[test]
fn a_fork_with_more_in_it_has_no_fix() {
    let (d, w) = pair(PULL);
    let d = with(
        &d,
        "        P[i].prefill (input);",
        "        fork { P[i].prefill (input); set t9 = now; }",
    );
    let r = refused(&d, &w, Code::PullForked);
    assert!(r.fixes.is_empty());
}

/// A reader with no connector: what it loads is the program's, no fix.
#[test]
fn a_missing_connector_has_no_fix() {
    let (d, w) = pair(PULL);
    let d = with(
        &with(
            &d,
            "      connector P { load (request.preempted ? 0 : request.input - request.hit); }\n",
            "",
        ),
        "        observe lease = D.load_done - t1;\n",
        "",
    );
    let r = refused(&d, &w, Code::CopyNoConnector);
    assert!(r.fixes.is_empty());
}
