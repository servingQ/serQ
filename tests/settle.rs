//! Every instant settles, every iteration lasts, and a session's marks are
//! its own: the three rules `docs/design/stochastic-model.md` §6 asked for
//! after writing the process down (Lemma 1, Definition 5, Proposition 1).
//!
//! Before them, `loop { run tool (w); }` with `w = 0` ran forever inside one
//! instant, a `cost` of 0 on an iteration with tokens made time stand still,
//! and the marks a session drew depended on how many draws other sessions
//! had made before it, so two deployments under one seed compared
//! different workloads.

mod common;

use std::collections::BTreeMap;

use serq::{Overrides, compile_source, run_source};

fn check(src: &str) -> Result<(), String> {
    compile_source(&common::main_source(src), &Overrides::default()).map(|_| ())
}

fn run(src: &str) -> Result<serq::Report, String> {
    run_source(&common::main_source(src), &Overrides::default(), None)
}

const TOOL: &str = "stage tool : delay;";
const CLIENT: &str = "arrive batch(1); init { set w = 0; }";

// --------------------------------------------------------- the linker ----

/// A loop whose body never runs, waits or ends is refused at link time.
#[test]
fn a_loop_that_never_lets_time_pass_is_a_link_error() {
    let e = check(&format!(
        "{TOOL} workload {{ {CLIENT} session {{ request; \n}} }}\nserver {{ loop {{ set w = w + 1; }}\n}} run {{ horizon 10; }}"
    ))
    .expect_err("refused");
    assert!(e.contains("let time pass"), "{e}");
}

/// A `run` on one arm of a `branch` does not cover the other arm.
#[test]
fn a_run_on_one_arm_only_is_a_link_error() {
    let e = check(&format!(
        "{TOOL} workload {{ {CLIENT} session {{ request;
        }} }}
        server {{ loop {{ branch (w > 0) {{ run tool (w); }} else {{ set w = w; }} }}
        }}
        run {{ horizon 10; }}"
    ))
    .expect_err("refused");
    assert!(e.contains("let time pass"), "{e}");
}

/// A `run` of constant zero work does not count: it completes at once.
#[test]
fn a_run_of_constant_zero_work_does_not_count() {
    let e = check(&format!(
        "{TOOL} workload {{ {CLIENT} session {{ request; \n}} }}\nserver {{ loop {{ run tool (0); }}\n}} run {{ horizon 10; }}"
    ))
    .expect_err("refused");
    assert!(e.contains("let time pass"), "{e}");
}

/// A `hold` whose body runs covers the loop; one whose body does not,
/// does not (it is admitted and released at one instant).
#[test]
fn a_hold_counts_only_through_its_body() {
    let pool = "pool kv { cap 100; }";
    check(&format!(
        "{pool} {TOOL} workload {{ {CLIENT} session {{ request; \n}} }}\nserver {{ loop {{ hold kv (1) {{ run tool (1); }} }}\n}} run {{ horizon 10; }}"
    ))
    .expect("the body runs");
    let e = check(&format!(
        "{pool} {TOOL} workload {{ {CLIENT} session {{ request; \n}} }}\nserver {{ loop {{ hold kv (1) {{ set w = 1; }} }}\n}} run {{ horizon 10; }}"
    ))
    .expect_err("refused");
    assert!(e.contains("let time pass"), "{e}");
}

/// `end` on both arms, or after the branch, covers the loop.
#[test]
fn end_and_a_later_run_cover_the_loop() {
    check(&format!(
        "{TOOL} workload {{ {CLIENT} session {{ request; loop {{ branch (w > 0) {{ end; }} else {{ end; }} }} \n}} }}\nserver {{\n}} run {{ horizon 10; }}"
    ))
    .expect("ends");
    check(&format!(
        "{TOOL} workload {{ {CLIENT} session {{ request;
        }} }}
        server {{ loop {{ branch (w > 0) {{ set w = 0; }} else {{ set w = 1; }} run tool (1); }}
        }}
        run {{ horizon 10; }}"
    ))
    .expect("runs after the branch");
}

// ------------------------------------------------------- the run time ----

/// A zero work the linker cannot see is caught when the instant does not
/// settle, instead of hanging.
#[test]
fn a_computed_zero_work_in_a_loop_is_a_run_time_error() {
    let e = run(&format!(
        "{TOOL} workload {{ {CLIENT} session {{ request; \n}} }}\nserver {{ loop {{ run tool (w); }}\n}} run {{ horizon 10; }}"
    ))
    .expect_err("does not settle");
    assert!(e.contains("does not settle"), "{e}");
}

/// A hold that fits, grows past the pool and preempts itself re-enters its
/// queue and is admitted again at the same instant, forever; now an error.
#[test]
fn a_self_preempting_grow_is_a_run_time_error() {
    let e = run("pool kv { cap 100; preempt lifo; }
        stage tool : delay;
        workload { arrive batch(1);
          session { request; end;
          }
        }
        server { hold kv (10) { grow kv (1000); run tool (1); }
        }
        run { horizon 10; }")
    .expect_err("does not settle");
    assert!(e.contains("does not settle"), "{e}");
}

/// An iteration that schedules tokens lasts a positive time.
#[test]
fn an_iteration_with_tokens_and_zero_cost_is_an_error() {
    let e = run("pool kv { cap 1000; block 16; evict lru; }
        stage engine : step { budget 512; cost 0; memory kv; }
        workload { arrive batch(1);
          session { request; end;
          }
        }
        server { hold kv (100) { prefill (100) growing kv; }
        }
        run { horizon 10; }")
    .expect_err("zero cost");
    assert!(e.contains("positive time"), "{e}");
}

/// The step that only preempted may still cost 0 (`docs/language.md` §3,
/// `tests/pool_semantics.rs`): the rule is about iterations with tokens.
#[test]
fn a_preempt_only_step_may_cost_zero() {
    run("pool reqs { cap 4; admit via engine; }
        pool kv { cap 160; block 16; evict lru; preempt lifo; }
        stage engine : step { budget 1000; chunk 0; cost tokens; memory kv; }
        workload { arrive batch(1);
          session { request;
            end;

          }
        }
        server {
          hold reqs (1), kv (100) reserve (100) {
            run engine prefill (100) growing kv;
            run engine decode (100) growing kv;
          }
        }
        run { horizon 400; }")
    .expect("runs to the horizon");
}

// --------------------------------------------- common random numbers ----

/// Two engines under one seed see the same workload: every (session, turn)
/// draws the same prompt whatever the budget did to the schedule. Before
/// per-session streams, 967 of 4 733 pairs differed on this program.
#[test]
fn the_marks_of_a_session_turn_do_not_depend_on_the_machine() {
    let prog = |budget: u32| {
        format!(
            "let B = {budget};
        pool kv {{ cap 1e6; block 16; evict lru; preempt lifo; }}
        pool reqs {{ cap 16; admit via engine; }}
        stage engine : step {{ budget B; cost 1e-4 + 1e-5 * tokens; memory kv; }}
        stage tool : delay;
        workload {{
          arrive poisson(0.3);
          init {{ set K = 0; set u0 = ~exp(1); }}
          turn {{ set n = K == 0 ? ~uniform(1000, 3000) : ~exp(500);
            set o = ~exp(200) + 1; set more = ~bernoulli(0.9); }}
          session {{
            turn;
            loop {{
              observe nn = n; observe oo = o; observe uu = u0;
              request;
              set K = prompt + o;
              branch (more) {{ tool (~exp(3)); turn; }} else {{ end; }}
            }}
          }}
        }}
        server {{
          set prompt = K + n;
          hold reqs (1), kv (prompt) {{
            prefill (prompt) growing kv;
            decode (o - 1) growing kv;
          }} cache (prompt + o);
        }}
        run {{ horizon 300; warmup 0; seed 1; }}"
        )
    };
    let marks = |r: &serq::Report, name: &str| -> BTreeMap<(u64, u32), f64> {
        let o = r.observe(name).unwrap();
        o.records
            .iter()
            .zip(&o.samples)
            .map(|(&(_, serial, turn), &v)| ((serial, turn), v))
            .collect()
    };
    let wide = run(&prog(8192)).expect("runs");
    let narrow = run(&prog(512)).expect("runs");
    for name in ["nn", "oo", "uu"] {
        let a = marks(&wide, name);
        let b = marks(&narrow, name);
        let common: Vec<_> = a.keys().filter(|k| b.contains_key(k)).collect();
        assert!(
            common.len() > 100,
            "{name}: only {} common (session, turn)",
            common.len()
        );
        let differ = common.iter().filter(|k| a[k] != b[k]).count();
        assert_eq!(
            differ,
            0,
            "{name}: {differ} of {} pairs differ",
            common.len()
        );
    }
}

/// The schedules themselves differ (the budget matters), so the agreement
/// above is not two identical runs.
#[test]
fn the_machine_still_matters() {
    let src = |budget: u32| {
        format!(
            "let B = {budget};
        pool kv {{ cap 1e6; block 16; evict lru; }}
        stage engine : step {{ budget B; cost 1e-4 + 1e-5 * tokens; memory kv; }}
        workload {{ arrive poisson(0.5); turn {{ set n = ~uniform(1000, 3000); }}
          session {{ turn; request; end;
          }}
        }}
        server {{ set t0 = now; hold kv (n) {{ prefill (n) growing kv; }}
          observe ttft = now - t0;
        }}
        run {{ horizon 200; warmup 0; seed 1; }}"
        )
    };
    let a = run(&src(8192)).expect("runs").observe("ttft").unwrap().mean;
    let b = run(&src(512)).expect("runs").observe("ttft").unwrap().mean;
    assert!(a < b, "a wider budget prefills faster: {a} vs {b}");
}

// ------------------------------------------------------------ bounds ----

/// The re-ready bound is per session: a batch of two thousand sessions that
/// all end at t = 0 is one busy instant, not a loop (the bound once scaled
/// with the live count, which fell as they ended, and refused this).
#[test]
fn a_large_batch_that_ends_at_once_settles() {
    let r = run("let N = 2000;
        stage tool : delay;
        workload { arrive batch(N); init { set w = ~uniform(0, 1); }
          session { request; end;
          }
        }
        server { observe w = w;
        }
        run { horizon 10; }")
    .expect("settles");
    assert_eq!(r.observe("w").unwrap().count, 2000);
}

/// A hold's header is re-read at every admission attempt, so it may not
/// draw: a draw there would move the session's later draws with the
/// machine (127 of 183 sessions did, on a cap of 10 000 against 150).
#[test]
fn a_hold_header_may_not_draw() {
    for (what, header) in [
        ("units", "kv (~uniform(1, 100))"),
        ("`reserve`", "kv (10) reserve (~uniform(1, 100))"),
        ("`reuse`", "kv (10) reuse (~uniform(0, 10))"),
    ] {
        let e = check(&format!(
            "pool kv {{ cap 1000; }} stage tool : delay; workload {{ arrive batch(1);
          session {{ request; end;
          }}
        }}
        server {{ hold {header} {{ run tool (1); }} cache (5);
        }} run {{ horizon 10; }}"
        ))
        .expect_err(what);
        assert!(
            e.contains(&format!("a hold's {what} may not draw")),
            "{what}: {e}"
        );
    }
}

/// The turn stream is keyed by the interpreter's count of turns, not by
/// the attribute `turn_no`, which a program may overwrite: a session that
/// resets `turn_no` still draws fresh marks every turn.
#[test]
fn overwriting_turn_no_does_not_repeat_the_marks() {
    let r = run("stage tool : delay;
        workload { arrive batch(1); turn { set n = ~uniform(0, 1); }
          session { turn; loop { request; turn; }
          }
        }
        server { observe nn = n; set turn_no = 0; run tool (1);
        }
        run { horizon 5; }")
    .expect("runs");
    let s = &r.observe("nn").unwrap().samples;
    assert!(s.len() >= 4, "{s:?}");
    assert!(s.windows(2).all(|w| w[0] != w[1]), "{s:?}");
}
