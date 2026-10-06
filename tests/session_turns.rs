//! Sessions describe completed turns and their continuation, independently
//! of the one server that handles each turn.
mod common;
use serq::{compile_source, run_source};

fn compile(s: &str) -> Result<serq::ir::Program, String> {
    compile_source(&common::main_source(s), &common::horizon(20.0))
}
fn run(s: &str) -> serq::Report {
    run_source(&common::main_source(s), &common::horizon(20.0), None).unwrap()
}

#[test]
fn an_omitted_session_is_one_complete_turn() {
    let base = "stage svc : delay;
      workload { arrive batch(1); turn { set s = 3; } SESSION }
      server { run svc (s); observe finished = now; observe ordinal = turn_no; }
      ";
    let implicit = base.replace("SESSION", "");
    let explicit = base.replace("SESSION", "session { turn; }");
    assert_eq!(
        compile(&implicit).unwrap().to_json(),
        compile(&explicit).unwrap().to_json()
    );
    let rep = run(&implicit);
    // One request of duration 3, numbered 1 even without a session declaration.
    assert_eq!(rep.observe("finished").unwrap().mean, 3.0);
    assert_eq!(rep.observe("ordinal").unwrap().mean, 1.0);
}

#[test]
fn turns_wait_for_responses_and_accumulate_the_next_input() {
    let rep = run("stage svc : delay; stage think : delay;
      workload {
        arrive batch(1); init { set k = 1; }
        turn { set s = k; }
        session {
          turn;
          while (turn_no < 3) { set k = k + s; run think (2); turn; }
          observe finished = now;
          observe history = k;
        }
      }
      server { run svc (s); observe served = s; }
      ");
    // Requests cost 1, 2, 4; two think intervals cost 2 each: 11 total.
    assert_eq!(rep.observe("finished").unwrap().mean, 11.0);
    assert_eq!(rep.observe("history").unwrap().mean, 4.0);
    assert_eq!(rep.observe("served").unwrap().mean, 7.0 / 3.0);
}

#[test]
fn nested_whiles_can_skip_and_continue_after_releasing_a_hold() {
    let rep = run("pool p { cap 1; } stage svc : delay;
      workload { arrive batch(1); session {
        hold p (1) {
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
      } }
      server { run svc (1); }
      ");
    // 2 x (1 + 2) requests; the false loop sends none and the scoped hold releases.
    assert_eq!(rep.observe("finished").unwrap().mean, 6.0);
    assert_eq!(rep.observe("allocated").unwrap().mean, 0.0);
}

#[test]
fn while_progress_is_checked_in_text_and_direct_ir() {
    let source = "stage svc : delay; workload { arrive batch(1); session {
      turn; while (1) { set x = 1; }
    } } server { run svc (1); } ";
    assert!(
        compile(source)
            .unwrap_err()
            .contains("a `while` must let time pass")
    );
    let mut p = compile(&source.replace("set x = 1;", "turn;")).unwrap();
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
    let source = "stage svc : delay; workload { arrive batch(1); session {
      while (0.5) { turn; }
    } } server { run svc (1); } ";
    assert!(compile(source).unwrap_err().contains("not 0 or 1"));
    let dynamic = source.replace("while (0.5)", "set probability = 0.5; while (probability)");
    let error =
        run_source(&common::main_source(&dynamic), &common::horizon(20.0), None).unwrap_err();
    let mut p = compile(&source.replace("0.5", "1")).unwrap();
    if let serq::ir::CStmt::While(guard, _) = &mut p.blocks[p.session][0] {
        *guard = serq::ir::CExpr::Num(0.5);
    } else {
        panic!("the session starts with its conditional loop");
    }
    assert!(p.validate().unwrap_err().contains("not 0 or 1"));
    assert!(error.contains("not 0 or 1"), "{error}");
    assert!(
        compile(&source.replace("while (0.5) { turn; }", "set x = 1;"))
            .unwrap_err()
            .contains("session has no `turn;`")
    );
}

#[test]
fn a_default_session_cannot_silently_omit_its_server() {
    assert!(
        compile("workload { arrive batch(1); } ")
            .unwrap_err()
            .contains("workload needs a `server`")
    );
}

#[test]
fn a_while_guard_obeys_the_enclosing_cache_contract() {
    let error = compile(
        "pool p { cap 1; } stage svc : delay;
      workload { arrive batch(1); }
      server { hold p (1) { while (cached > 0) { run svc (1); } } }
      ",
    )
    .unwrap_err();
    assert!(error.contains("has no `cache` clause"), "{error}");
}
