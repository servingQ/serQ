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
    compile_source(&common::main_source(src), &common::horizon(10.0)).map(|_| ())
}

fn run(src: &str, options: &Overrides) -> Result<serq::Report, String> {
    run_source(&common::main_source(src), options, None)
}

const TOOL: &str = "stage tool : delay;";
const CLIENT: &str = "arrive batch(1); init { set w = 0; }";

// --------------------------------------------------------- the linker ----

/// A loop whose body never runs, waits or ends is refused at link time.
#[test]
fn a_loop_that_never_lets_time_pass_is_a_link_error() {
    let e = check(&format!(
        "{TOOL} workload {{ {CLIENT} session {{ turn; \n}} }}\nserver {{ loop {{ set w = w + 1; }}\n}} "
    ))
    .expect_err("refused");
    assert!(e.contains("let time pass"), "{e}");
}

/// A `run` on one arm of an `if` does not cover the other arm.
#[test]
fn a_run_on_one_arm_only_is_a_link_error() {
    let e = check(&format!(
        "{TOOL} workload {{ {CLIENT} session {{ turn;
        }} }}
        server {{ loop {{ if (w > 0) {{ run tool (cost(tool, w)); }} else {{ set w = w; }} }}
        }}
        "
    ))
    .expect_err("refused");
    assert!(e.contains("let time pass"), "{e}");
}

/// A `run` of constant zero work does not count: it completes at once.
#[test]
fn a_run_of_constant_zero_work_does_not_count() {
    let e = check(&format!(
        "{TOOL} workload {{ {CLIENT} session {{ turn; \n}} }}\nserver {{ loop {{ run tool (cost(tool, 0)); }}\n}} "
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
        "{pool} {TOOL} workload {{ {CLIENT} session {{ turn; \n}} }}\nserver {{ loop {{ hold kv (cost(kv, 1)) {{ run tool (cost(tool, 1)); }} }}\n}} "
    ))
    .expect("the body runs");
    let e = check(&format!(
        "{pool} {TOOL} workload {{ {CLIENT} session {{ turn; \n}} }}\nserver {{ loop {{ hold kv (cost(kv, 1)) {{ set w = 1; }} }}\n}} "
    ))
    .expect_err("refused");
    assert!(e.contains("let time pass"), "{e}");
}

/// `end` on both arms, or after the branch, covers the loop.
#[test]
fn end_and_a_later_run_cover_the_loop() {
    check(&format!(
        "{TOOL} workload {{ {CLIENT} session {{ turn; loop {{ if (w > 0) {{ end; }} else {{ end; }} }} \n}} }}\nserver {{\n}} "
    ))
    .expect("ends");
    check(&format!(
        "{TOOL} workload {{ {CLIENT} session {{ turn;
        }} }}
        server {{ loop {{ if (w > 0) {{ set flag = 0; }} else {{ set flag = 1; }} run tool (cost(tool, 1)); }}
        }}
        "
    ))
    .expect("runs after the branch");
}

// ------------------------------------------------------- the run time ----

/// A zero work the linker cannot see is caught when the instant does not
/// settle, instead of hanging.
#[test]
fn a_computed_zero_work_in_a_loop_is_a_run_time_error() {
    let e = run(&format!(
        "{TOOL} workload {{ {CLIENT} session {{ turn; \n}} }}\nserver {{ loop {{ run tool (cost(tool, w)); }}\n}} "
    ), &common::horizon(10.0))
    .expect_err("does not settle");
    assert!(e.contains("does not settle"), "{e}");
}

/// A hold that fits, grows past the pool and preempts itself re-enters its
/// queue and is admitted again at the same instant, forever; now an error.
#[test]
fn a_self_preempting_grow_is_a_run_time_error() {
    let e = run(
        "pool kv { cap 100; preempt lifo; }
        stage tool : delay;
        workload { arrive batch(1);
          session { turn; end;
          }
        }
        server { hold kv (cost(kv, 10)) { grow kv (cost(kv, 1000)); run tool (cost(tool, 1)); }
        }
        ",
        &common::horizon(10.0),
    )
    .expect_err("does not settle");
    assert!(e.contains("does not settle"), "{e}");
}

/// An iteration that schedules tokens lasts a positive time.
#[test]
fn an_iteration_with_tokens_and_zero_cost_is_an_error() {
    let e = run(
        "device gpu { kv cap 1000; }
        engine llm on gpu {
          tokens cap 512;
          request () { prefill (100) growing kv; }
          scheduler { advance running; admit waiting while (running.preempted == 0) taking kv (100); }
          execute (0);
        }
        pool kv on gpu { block 16; evict lru; }
        workload { arrive batch(1);
          session { turn; end;
          }
        }
        server { llm ();
        }
        ",
        &common::horizon(10.0),
    )
    .expect_err("zero cost");
    assert!(e.contains("positive time"), "{e}");
}

/// The step that only preempted may still cost 0 (`docs/language.md` §3,
/// `tests/pool_semantics.rs`): the rule is about iterations with tokens.
#[test]
fn a_preempt_only_step_may_cost_zero() {
    run(
        "device gpu { kv cap 160; }
        engine llm on gpu {
          reqs cap 4;
          tokens cap 1000;
          request () {
            prefill (100) growing kv;
            decode (100) growing kv;
          }
          scheduler {
            advance running;
            admit waiting while (running.preempted == 0) taking reqs (1), kv (100) reserve (100);
          }
          execute (batch.tokens);
        }
        pool reqs on llm { }
        pool kv on gpu { block 16; evict lru; preempt lifo; }
        workload { arrive batch(1);
          session { turn;
            end;

          }
        }
        server { llm (); }
        ",
        &common::horizon(400.0),
    )
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
        device gpu {{ kv cap 1e6; }}
        engine llm on gpu {{
          reqs cap 16;
          tokens cap B;
          request (prompt, output) {{
            prefill (prompt) growing kv;
            decode (output - 1) growing kv;
            cache (prompt + output);
          }}
          scheduler {{
            advance running;
            admit waiting while (running.preempted == 0) taking reqs (1), kv (prompt);
          }}
          execute (1e-4 + 1e-5 * batch.tokens);
        }}
        pool kv on gpu {{ block 16; evict lru; preempt lifo; }}
        pool reqs on llm {{ }}
        stage tool : delay;
        workload {{
          arrive poisson(0.3);
          init {{ set K = 0; set u0 = ~exp(1); }}
          turn {{ set n = K == 0 ? ~uniform(1000, 3000) : ~exp(500);
            set o = ~exp(200) + 1; set more = ~bernoulli(0.9); }}
          session {{

            loop {{
              observe nn = n; observe oo = o; observe uu = u0;
              turn;
              set K = prompt + o;
              if (more) {{ tool (~exp(3));  }} else {{ end; }}
            }}
          }}
        }}
        server {{
          set prompt = K + n;
          llm (prompt, o);
        }}
        "
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
    let wide = run(
        &prog(8192),
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(300.0)
        },
    )
    .expect("runs");
    let narrow = run(
        &prog(512),
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(300.0)
        },
    )
    .expect("runs");
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
        device gpu {{ kv cap 1e6; }}
        engine llm on gpu {{
          tokens cap B;
          request (n) {{ prefill (n) growing kv; }}
          scheduler {{ advance running; admit waiting while (running.preempted == 0) taking kv (n); }}
          execute (1e-4 + 1e-5 * batch.tokens);
        }}
        pool kv on gpu {{ block 16; evict lru; }}
        workload {{ arrive poisson(0.5); turn {{ set n = ~uniform(1000, 3000); }}
          session {{  turn; end;
          }}
        }}
        server {{ set t0 = now; llm (n);
          observe ttft = now - t0;
        }}
        "
        )
    };
    let a = run(
        &src(8192),
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(200.0)
        },
    )
    .expect("runs")
    .observe("ttft")
    .unwrap()
    .mean;
    let b = run(
        &src(512),
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(200.0)
        },
    )
    .expect("runs")
    .observe("ttft")
    .unwrap()
    .mean;
    assert!(a < b, "a wider budget prefills faster: {a} vs {b}");
}

/// Seeds name independent runs: seed 2's session k does not draw what seed
/// 1's session k+1 drew. A session's stream once added the seed and its
/// serial in the same splitmix64 step, so seed s+1's session k was seed s's
/// session k+1, and seeds 1..N were one sample path shifted by a session
/// each (#297: 39 of 39 sessions here).
#[test]
fn consecutive_seeds_do_not_replay_each_others_sessions() {
    let src = "stage svc : fifo;
        workload { arrive batch(40); session { set x = ~exp(1); turn; observe drawn = x; end; } }
        server { run svc (cost(svc, 1)); }";
    let draws = |seed: u64| -> std::collections::BTreeMap<u64, f64> {
        let overrides = Overrides {
            seed: Some(seed),
            ..common::horizon(1e3)
        };
        let r = run(src, &overrides).unwrap();
        let o = r.observe("drawn").unwrap();
        o.records
            .iter()
            .map(|r| r.1)
            .zip(o.samples.iter().copied())
            .collect()
    };
    let (one, two) = (draws(1), draws(2));
    assert_eq!(one.len(), 40);
    let replayed = two
        .iter()
        .filter(|&(k, x)| one.get(&(k + 1)) == Some(x))
        .count();
    assert_eq!(replayed, 0, "seed 2 replays seed 1 a session later");
}

// ------------------------------------------------------------ bounds ----

/// The re-ready bound is per session: a batch of two thousand sessions that
/// all end at t = 0 is one busy instant, not a loop (the bound once scaled
/// with the live count, which fell as they ended, and refused this).
#[test]
fn a_large_batch_that_ends_at_once_settles() {
    let r = run(
        "let N = 2000;
        stage tool : delay;
        workload { arrive batch(N); init { set w = ~uniform(0, 1); }
          session { turn; end;
          }
        }
        server { observe drawn = w;
        }
        ",
        &common::horizon(10.0),
    )
    .expect("settles");
    assert_eq!(r.observe("drawn").unwrap().count, 2000);
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
          session {{ turn; end;
          }}
        }}
        server {{ hold {header} {{ run tool (cost(tool, 1)); }} cache (5);
        }} "
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
    let r = run(
        "stage tool : delay;
        workload { arrive batch(1); turn { set n = ~uniform(0, 1); }
          session { loop { set turn_no = 0; turn; }
          }
        }
        server { observe nn = n; run tool (cost(tool, 1));
        }
        ",
        &common::horizon(5.0),
    )
    .expect("runs");
    let s = &r.observe("nn").unwrap().samples;
    assert!(s.len() >= 4, "{s:?}");
    assert!(s.windows(2).all(|w| w[0] != w[1]), "{s:?}");
}
