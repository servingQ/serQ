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

use serq::Overrides;

fn check(src: &str, workload: &str) -> Result<(), String> {
    common::compile(&common::main_source(src), workload, &common::horizon(10.0)).map(|_| ())
}

fn run(src: &str, workload: &str, options: &Overrides) -> Result<serq::Report, String> {
    common::run(&common::main_source(src), workload, options)
}

const TOOL: &str = "stage tool : delay;";
/// One session of one turn, sending `w = 0`.
const CLIENT: &str =
    "workload { arrive batch(1); init { set w = 0; } send (w); session { turn; } }";

// --------------------------------------------------------- the linker ----

/// A loop whose body never runs, waits or ends is refused at link time.
#[test]
fn a_loop_that_never_lets_time_pass_is_a_link_error() {
    let e = check(
        &format!("{TOOL}\nserver (w) {{ loop {{ set w = w + 1; }}\n}} "),
        CLIENT,
    )
    .expect_err("refused");
    assert!(e.contains("let time pass"), "{e}");
}

/// A `run` on one arm of an `if` does not cover the other arm.
#[test]
fn a_run_on_one_arm_only_is_a_link_error() {
    let e = check(
        &format!(
            "{TOOL}
        server (w) {{ loop {{ if (w > 0) {{ run tool (cost(tool, w)); }} else {{ set w = w; }} }}
        }}
        "
        ),
        CLIENT,
    )
    .expect_err("refused");
    assert!(e.contains("let time pass"), "{e}");
}

/// A `run` of constant zero work does not count: it completes at once.
#[test]
fn a_run_of_constant_zero_work_does_not_count() {
    let e = check(
        &format!("{TOOL}\nserver (w) {{ loop {{ run tool (cost(tool, 0)); }}\n}} "),
        CLIENT,
    )
    .expect_err("refused");
    assert!(e.contains("let time pass"), "{e}");
}

/// A `hold` whose body runs covers the loop; one whose body does not,
/// does not (it is admitted and released at one instant).
#[test]
fn a_hold_counts_only_through_its_body() {
    let pool = "pool kv { cap 100; }";
    check(
        &format!(
            "{pool} {TOOL}\nserver (w) {{ loop {{ hold kv (cost(kv, 1)) {{ run tool (cost(tool, 1)); }} }}\n}} "
        ),
        CLIENT,
    )
    .expect("the body runs");
    let e = check(
        &format!(
            "{pool} {TOOL}\nserver (w) {{ loop {{ hold kv (cost(kv, 1)) {{ set w = 1; }} }}\n}} "
        ),
        CLIENT,
    )
    .expect_err("refused");
    assert!(e.contains("let time pass"), "{e}");
}

/// `end` on both arms, or after the branch, covers the loop.
#[test]
fn end_and_a_later_run_cover_the_loop() {
    check(
        "server (w) {\n} ",
        "workload { arrive batch(1); init { set w = 0; } send (w);
          session { turn; loop { if (w > 0) { end; } else { end; } } } }",
    )
    .expect("ends");
    check(
        &format!(
            "{TOOL}
        server (w) {{ loop {{ if (w > 0) {{ set flag = 0; }} else {{ set flag = 1; }} run tool (cost(tool, 1)); }}
        }}
        "
        ),
        CLIENT,
    )
    .expect("runs after the branch");
}

// ------------------------------------------------------- the run time ----

/// A zero work the linker cannot see is caught when the instant does not
/// settle, instead of hanging.
#[test]
fn a_computed_zero_work_in_a_loop_is_a_run_time_error() {
    let e = run(
        &format!("{TOOL}\nserver (w) {{ loop {{ run tool (cost(tool, w)); }}\n}} "),
        CLIENT,
        &common::horizon(10.0),
    )
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
        server { hold kv (cost(kv, 10)) { grow kv (cost(kv, 1000)); run tool (cost(tool, 1)); }
        }
        ",
        "workload { arrive batch(1);
          session { turn; end;
          }
        }",
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
          scheduler { advance running; admit waiting while (running.preempted == 0) taking kv (100); }
          worker { forward (0); }
        }
        pool kv on gpu { block 16; evict lru; }
        server { llm (100, 1);
        }
        ",
        "workload { arrive batch(1);
          session { turn; end;
          }
        }",
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
          scheduler {
            advance running;
            admit waiting while (running.preempted == 0) taking reqs (1), kv (100) reserve (request.known);
          }
          worker { forward (batch.tokens); }
        }
        pool reqs on llm { }
        pool kv on gpu { block 16; evict lru; preempt lifo; }
        server { llm (100, 101); }
        ",
        "workload { arrive batch(1);
          session { turn;
            end;

          }
        }",
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
          scheduler {{
            advance running;
            admit waiting while (running.preempted == 0) taking reqs (1), kv (request.input)
              reuse (0) cache (request.input + request.output);
          }}
          worker {{ forward (1e-4 + 1e-5 * batch.tokens); }}
        }}
        pool kv on gpu {{ block 16; evict lru; preempt lifo; }}
        pool reqs on llm {{ }}
        server (K, n, o) {{
          set prompt = K + n;
          llm (prompt, o);
        }}
        "
        )
    };
    // the client's own time, `sleep`, on a delay of the workload's
    let workload = "workload {
          arrive poisson(0.3);
          init { set K = 0; set u0 = ~exp(1); }
          turn { set n = ceil(K == 0 ? ~uniform(1000, 3000) : ~exp(500));
            set o = floor(~exp(200)) + 1; set more = ~bernoulli(0.9); }
          send (K, n, o);
          session {

            loop {
              observe nn = n; observe oo = o; observe uu = u0;
              turn;
              set K = K + n + o;
              if (more) { sleep (~exp(3));  } else { end; }
            }
          }
        }";
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
        workload,
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(300.0)
        },
    )
    .expect("runs");
    let narrow = run(
        &prog(512),
        workload,
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
          scheduler {{ advance running; admit waiting while (running.preempted == 0) taking kv (request.input); }}
          worker {{ forward (1e-4 + 1e-5 * batch.tokens); }}
        }}
        pool kv on gpu {{ block 16; evict lru; }}
        server (n) {{ set t0 = now; llm (n, 1);
          observe ttft = now - t0;
        }}
        "
        )
    };
    let workload = "workload { arrive poisson(0.5); turn { set n = ceil(~uniform(1000, 3000)); }
          send (n);
          session {  turn; end;
          }
        }";
    let a = run(
        &src(8192),
        workload,
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
        workload,
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
        server { run svc (cost(svc, 1)); }";
    let workload =
        "workload { arrive batch(40); session { set x = ~exp(1); turn; observe drawn = x; end; } }";
    let draws = |seed: u64| -> std::collections::BTreeMap<u64, f64> {
        let overrides = Overrides {
            seed: Some(seed),
            ..common::horizon(1e3)
        };
        let r = run(src, workload, &overrides).unwrap();
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
        "server (w) { observe drawn = w;
        }
        ",
        "let N = 2000;
        workload { arrive batch(N); init { set w = ~uniform(0, 1); } send (w);
          session { turn; end;
          }
        }",
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
        let e = check(
            &format!(
                "pool kv {{ cap 1000; }} stage tool : delay;
        server {{ hold {header} {{ run tool (cost(tool, 1)); }} cache (5);
        }} "
            ),
            "workload { arrive batch(1);
          session { turn; end;
          }
        }",
        )
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
        server (n) { observe nn = n; run tool (cost(tool, 1));
        }
        ",
        "workload { arrive batch(1); turn { set n = ~uniform(0, 1); } send (n);
          session { loop { set turn_no = 0; turn; }
          }
        }",
        &common::horizon(5.0),
    )
    .expect("runs");
    let s = &r.observe("nn").unwrap().samples;
    assert!(s.len() >= 4, "{s:?}");
    assert!(s.windows(2).all(|w| w[0] != w[1]), "{s:?}");
}
