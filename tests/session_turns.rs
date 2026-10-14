//! Sessions describe completed turns and their continuation, independently
//! of the one server that handles each turn.
mod common;

fn compile(s: &str, w: &str) -> Result<serq::ir::Program, String> {
    common::compile(&common::main_source(s), w, &common::horizon(20.0))
}
fn run(s: &str, w: &str) -> serq::Report {
    common::run(&common::main_source(s), w, &common::horizon(20.0)).unwrap()
}

#[test]
fn an_omitted_session_is_one_complete_turn() {
    let deployment = "stage svc : delay;
      server (s, turn_no) { run svc (cost(svc, s)); observe finished = now; observe ordinal = turn_no; }
      ";
    let base = "workload { arrive batch(1); turn { set s = 3; } send (s, turn_no); SESSION }";
    let implicit = base.replace("SESSION", "");
    let explicit = base.replace("SESSION", "session { turn; }");
    assert_eq!(
        compile(deployment, &implicit).unwrap().to_json(),
        compile(deployment, &explicit).unwrap().to_json()
    );
    let rep = run(deployment, &implicit);
    // One request of duration 3, numbered 1 even without a session declaration.
    assert_eq!(rep.observe("finished").unwrap().mean, 3.0);
    assert_eq!(rep.observe("ordinal").unwrap().mean, 1.0);
}

#[test]
fn turns_wait_for_responses_and_accumulate_the_next_input() {
    let rep = run(
        "stage svc : delay;
      server (s) { run svc (cost(svc, s)); observe service = s; }
      ",
        "stage user : delay;
      workload {
        arrive batch(1); init { set k = 1; }
        turn { set s = k; }
        send (s);
        session {
          turn;
          while (turn_no < 3) { set k = k + s; run user (cost(user, 2)); turn; }
          observe finished = now;
          observe history = k;
        }
      }",
    );
    // Requests cost 1, 2, 4; two think intervals cost 2 each: 11 total.
    assert_eq!(rep.observe("finished").unwrap().mean, 11.0);
    assert_eq!(rep.observe("history").unwrap().mean, 4.0);
    assert_eq!(rep.observe("service").unwrap().mean, 7.0 / 3.0);
}

#[test]
fn nested_whiles_can_skip_and_continue_after_releasing_a_hold() {
    // the pool the session holds is the workload's own
    let rep = run(
        "stage svc : delay;
      server { run svc (cost(svc, 1)); }
      ",
        "pool p { cap 1; }
      workload { arrive batch(1); session {
        hold p (cost(p, 1)) {
          set outer = 0;
          while (outer < 2) {
            turn;
            set inner = 0;
            while (inner < 2) { turn; set inner = inner + 1; }
            set outer = outer + 1;
          }
          while (0) { turn; }
        }
        observe finished = now;
        observe allocated = used(p);
      } }",
    );
    // 2 x (1 + 2) requests; the false loop sends none and the scoped hold releases.
    assert_eq!(rep.observe("finished").unwrap().mean, 6.0);
    assert_eq!(rep.observe("allocated").unwrap().mean, 0.0);
}

const SVC: &str = "stage svc : delay; server { run svc (cost(svc, 1)); } ";

#[test]
fn while_progress_is_checked_in_text_and_direct_ir() {
    let workload = "workload { arrive batch(1); session {
      turn; while (1) { set x = 1; }
    } }";
    assert!(
        compile(SVC, workload)
            .unwrap_err()
            .contains("a `while` must let time pass")
    );
    let mut p = compile(SVC, &workload.replace("set x = 1;", "turn;")).unwrap();
    let body = p.blocks[p.session]
        .iter()
        .find_map(|s| match s {
            serq::ir::CStmt::While(_, b) => Some(*b),
            _ => None,
        })
        .unwrap();
    p.blocks[body].clear();
    assert!(
        p.validate()
            .unwrap_err()
            .contains("a `while` must let time pass")
    );
}

#[test]
fn while_guards_are_tests_and_missing_turns_are_diagnosed() {
    let workload = "workload { arrive batch(1); session {
      while (0.5) { turn; }
    } }";
    assert!(compile(SVC, workload).unwrap_err().contains("not 0 or 1"));
    let dynamic = workload.replace("while (0.5)", "set probability = 0.5; while (probability)");
    let error =
        common::run(&common::main_source(SVC), &dynamic, &common::horizon(20.0)).unwrap_err();
    let mut p = compile(SVC, &workload.replace("0.5", "1")).unwrap();
    if let serq::ir::CStmt::While(guard, _) = &mut p.blocks[p.session][0] {
        *guard = serq::ir::CExpr::Num(0.5);
    } else {
        panic!("the session starts with its conditional loop");
    }
    assert!(p.validate().unwrap_err().contains("not 0 or 1"));
    assert!(error.contains("not 0 or 1"), "{error}");
    assert!(
        compile(
            SVC,
            &workload.replace("while (0.5) { turn; }", "set x = 1;")
        )
        .unwrap_err()
        .contains("session has no `turn;`")
    );
}

/// The single-file form this guarded (a workload in `fn main` beside no
/// server) is refused outright now: the workload is a file of its own.
#[test]
fn a_default_session_cannot_silently_omit_its_server() {
    let error = serq::compile_source(
        &common::main_source("workload { arrive batch(1); } "),
        &common::horizon(20.0),
    )
    .unwrap_err();
    assert!(error.contains("a workload is a file of its own"), "{error}");
    let error = compile("stage svc : delay;", "workload { arrive batch(1); }").unwrap_err();
    assert!(error.contains("workload needs a `server`"), "{error}");
}

#[test]
fn a_while_guard_obeys_the_enclosing_cache_contract() {
    let error = compile(
        "pool p { cap 1; } stage svc : delay;
      server { hold p (cost(p, 1)) { while (cached > 0) { run svc (cost(svc, 1)); } } }
      ",
        "workload { arrive batch(1); }",
    )
    .unwrap_err();
    assert!(error.contains("has no `cache` clause"), "{error}");
}
