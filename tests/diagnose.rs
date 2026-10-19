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
        "        observe send_held = D.load_done - t1;\n",
        "",
    );
    let r = refused(&d, &w, Code::CopyNoConnector);
    assert!(r.fixes.is_empty());
}

/// How much a pool holds is the program's: no fix-it (#645 A4).
#[test]
fn a_pool_with_no_capacity_has_no_fix() {
    let (d, w) = pair(PUSH);
    let d = with(&d, "pool reqs on D {", "pool req on D {");
    let r = refused(&d, &w, Code::PoolNoCapacity);
    assert!(r.message.contains("did you mean `reqs`?"), "{}", r.message);
    assert!(r.fixes.is_empty());
}

/// A relation written reader first (#645 A6): the fix-it swaps the two
/// names, and the program is as it was written before.
#[test]
fn a_swapped_relation_is_swapped_back() {
    let (d, w) = pair(PUSH);
    let swapped = with(&d, "kvt.nixl_push (P, D,", "kvt.nixl_push (D, P,");
    let r = refused(&swapped, &w, Code::CopySwapped);
    assert_eq!(r.fixes.len(), 1, "{}", r.message);
    assert_eq!(fixed(&swapped, &r.fixes[0]), d);
}

/// A reader's connector named for another queue than the one its relation
/// reads from (#645 A7): the relation chose the source, so the rename makes
/// no choice, and the program is as it was written before.
#[test]
fn a_misnamed_connector_is_renamed() {
    let (d, w) = pair(PUSH);
    let wrong = with(
        &d,
        "      connector P { load (",
        "      connector gw { load (",
    );
    let r = refused(&wrong, &w, Code::ConnectorMisnamed);
    let range = r.range.unwrap();
    assert_eq!(&wrong[range.0..range.1], "gw");
    assert_eq!(r.fixes.len(), 1, "{}", r.message);
    assert_eq!(fixed(&wrong, &r.fixes[0]), d);
}
