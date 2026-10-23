//! A queue that admits at its ticks (`queue … every (dt)`,
//! `docs/design/thunderagent.md`): the pool admits its waiting holds only
//! at every multiple of `dt`, in the queue's order, each admitted one
//! running until it blocks before the next is tried; the first that does
//! not fit ends the tick, unless the queue says `first fit`
//! (`docs/design/first-fit.md`), which passes it over. A hold queued during
//! a tick waits for the next. ThunderAgent's global waiting queue, drained
//! at the router's tick.

mod common;

/// `n` sessions arrive at t = 0, each one turn.
fn sessions(n: usize) -> String {
    format!("workload {{ arrive batch({n}); session {{ turn; }} }}\n")
}

fn run(n: usize, src: &str) -> serq::Report {
    run_to(n, src, 200.0)
}

fn run_to(n: usize, src: &str, horizon: f64) -> serq::Report {
    common::run(
        &common::main_source(src),
        &sessions(n),
        &common::horizon(horizon),
    )
    .unwrap()
}

/// Three sessions queue at t = 0 and are let in at the tick at t = 5, the
/// greatest serial first. Each one let in takes a unit of `room` before
/// the next is tried: they read `free(room)` 2, 1 and 0.
#[test]
fn a_tick_lets_the_waiters_in_in_its_order_one_after_another() {
    let r = run(
        3,
        r#"
        stage svc : fifo(4);
        pool gate { queue by (-serial) every (5); }
        pool room { cap 2; }
        server {
          hold gate (cost(gate, 1)) { observe at = now; observe who = serial; observe room_left = free(room); }
          hold room (cost(room, 1)) { run svc (cost(svc, 100)); } lease room (inf);
        }
"#,
    );
    assert_eq!(r.observe("at").unwrap().samples, vec![5.0, 5.0, 5.0]);
    assert_eq!(r.observe("who").unwrap().samples, vec![2.0, 1.0, 0.0]);
    assert_eq!(r.observe("room_left").unwrap().samples, vec![2.0, 1.0, 0.0]);
}

/// Serial 2 takes 1 of 2 units at the tick at t = 5, and serial 1 asks 2
/// of the 1 left.
fn tick_with_a_misfit(fit: &str) -> serq::Report {
    run_to(
        3,
        &format!(
            r#"
        stage svc : fifo(4);
        pool gate {{ cap 2; queue by (-serial){fit} every (5); }}
        server {{
          hold gate (cost(gate, serial == 1 ? 2 : 1)) {{
            observe who = serial;
            observe at = now;
            run svc (cost(svc, 101));
          }}
        }}
"#
        ),
        300.0,
    )
}

/// Under `first fit` a waiter that does not fit is passed over, not waited
/// behind: serial 1 asks 2 of the 1 that serial 2 leaves, and serial 0 goes
/// in before it. Serial 1 goes in at the first tick after the others leave
/// at t = 106, t = 110.
#[test]
fn a_waiter_that_does_not_fit_is_passed_over() {
    let r = tick_with_a_misfit(" first fit");
    assert_eq!(r.observe("who").unwrap().samples, vec![2.0, 0.0, 1.0]);
    assert_eq!(r.observe("at").unwrap().samples, vec![5.0, 5.0, 110.0]);
}

/// Without `first fit` the first that does not fit ends the tick, as it
/// stops a pool's admission at any other time: serial 0 waits behind
/// serial 1, which goes in at t = 110 when serial 2 has left at t = 106,
/// and serial 0 at t = 215, when serial 1 has left at t = 211.
#[test]
fn without_first_fit_a_tick_stops_at_the_first_that_does_not_fit() {
    let r = tick_with_a_misfit("");
    assert_eq!(r.observe("who").unwrap().samples, vec![2.0, 1.0, 0.0]);
    assert_eq!(r.observe("at").unwrap().samples, vec![5.0, 110.0, 215.0]);
}

/// A hold queued during a tick waits for the next: a session that queues
/// again from inside the tick goes in at t = 5, then at t = 10. So a
/// `while` around such a hold lets time pass, and links.
#[test]
fn a_hold_queued_during_a_tick_waits_for_the_next() {
    let r = run(
        1,
        r#"
        stage svc : fifo(1);
        pool gate { queue fifo every (5); }
        server {
          set m = 0;
          while (m < 2) {
            hold gate (cost(gate, 1)) { }
            set m = m + 1;
            observe at = now;
          }
          run svc (cost(svc, 1));
        }
"#,
    );
    assert_eq!(r.observe("at").unwrap().samples, vec![5.0, 10.0]);
}

#[test]
fn a_queue_tick_is_positive_and_no_engines() {
    let refused = |src: &str, needle: &str| {
        let e = common::compile(
            &common::main_source(src),
            &sessions(1),
            &common::horizon(10.0),
        )
        .unwrap_err();
        assert!(e.contains(needle), "{e}");
    };
    refused(
        r#"
        stage svc : fifo(1);
        pool gate { queue fifo every (0); }
        server { hold gate (cost(gate, 1)) { run svc (cost(svc, 1)); } }
"#,
        "a tick is a positive, finite number of seconds",
    );
    refused(
        r#"
        device gpu { kv cap 64; }
        engine llm on gpu {
          tokens cap 8;
          scheduler { advance running; admit waiting taking kv (8); }
          worker { forward (1); }
        }
        pool kv on llm.gpu { queue fifo every (5); }
        server { llm (8, 1); }
"#,
        "`queue … every` on a pool an engine admits",
    );
}

/// Found in review (#591): a queue's tick ran each admission to the
/// instant's end, where an engine's iteration starts, so the first waiter
/// let in started one alone and the second waited for the next. Both are
/// let in at t = 5 and finish in the same iteration, at t = 6.
#[test]
fn the_waiters_of_one_tick_reach_the_same_iteration() {
    let r = run(
        2,
        r#"
        device gpu { kv cap 64; }
        engine llm on gpu {
          tokens cap 64;
          scheduler { advance running; admit waiting taking kv (8); }
          worker { forward (1); }
        }
        pool kv on llm.gpu { }
        pool gate { queue fifo every (5); }
        server {
          hold gate (cost(gate, 1)) { }
          llm (8, 1);
          observe done = now;
        }
"#,
    );
    assert_eq!(r.observe("done").unwrap().samples, vec![6.0, 6.0]);
}

/// The pools' ticks at an instant go in the order the pools are declared.
/// Serials 0 and 1 lease 8 and 4 of 10 by t = 0; serial 2 waits for the
/// queue's tick at t = 5, where `prog` also pauses serial 0. Declared
/// first, the queue lets serial 2 in before the pause (it reads `free`
/// -2); declared after, after it (6).
#[test]
fn the_pools_ticks_go_in_declaration_order() {
    let seen = |pools: &str| {
        let src = format!(
            r#"
            stage svc : fifo(3);
            {pools}
            server {{
              if (serial < 2) {{
                hold prog (cost(prog, serial == 0 ? 4 : 2)) {{ }} lease prog (inf);
                grow prog (cost(prog, serial == 0 ? 4 : 2));
                run svc (cost(svc, 20));
              }} else {{
                hold gate (cost(gate, 1)) {{ observe seen = free(prog); }}
                run svc (cost(svc, 1));
              }}
            }}
"#
        );
        run(3, &src).observe("seen").unwrap().samples[0]
    };
    let gate = "pool gate { queue fifo every (5); }";
    let prog = "pool prog { cap 10; preempt leases by (serial) every (5) suspend by (serial); }";
    assert_eq!(seen(&format!("{gate} {prog}")), -2.0);
    assert_eq!(seen(&format!("{prog} {gate}")), 6.0);
}
