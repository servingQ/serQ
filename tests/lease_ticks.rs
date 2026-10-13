//! A pool that acts at its ticks (`docs/design/thunderagent.md`): under
//! `preempt leases by (keys) every (dt)` a lease's growth is let through at
//! once, past the cap if it must, and at every multiple of `dt` the pool
//! ends leases in the keys' order while it is over its cap. With `suspend`
//! a lease the tick takes is set aside, and a later tick gives it back if
//! it fits, before the lessee asks again. ThunderAgent's scheduler loop:
//! `_pause_until_safe` at its tick, and `_greedy_resume` of a paused
//! program that has no request pending.

mod common;

use serq::{check_source, run_source};

/// Two programs lease 4 of 10 units. Serial 1 comes back at t = 2 and
/// grows its lease by 4: 12 of 10, let through until the tick at t = 5,
/// which takes serial 0's (the least key). Serial 1 gives its lease back
/// at t = 7. Serial 0 is at its tool until t = 11.
fn two(preempt: &str) -> String {
    format!(
        r#"
        stage svc : fifo(2);
        pool prog {{ cap 10; preempt leases by (serial) {preempt}; }}
        workload {{ arrive batch(2);
          session {{ turn; tool (serial == 0 ? 10 : 1); turn; tool 4; turn; }}
        }}
        server {{
          if (serial == 1 && turn_no == 3) {{ release prog; end; }}
          if (allocated(prog) > 0) {{
            if (turn_no == 2) {{
              grow prog (cost(prog, 4));
              observe grown = allocated(prog);
              observe free_after = free(prog);
            }}
          }} else {{
            if (turn_no > 1) {{ observe again = now; }}
            hold prog (cost(prog, 4)) {{ observe let_in = now; }} lease prog (inf);
          }}
          run svc (cost(svc, 1));
        }}
"#
    )
}

fn run(preempt: &str) -> serq::Report {
    run_source(
        &common::main_source(&two(preempt)),
        &common::horizon(100.0),
        None,
    )
    .unwrap()
}

#[test]
fn a_growth_goes_past_the_cap_until_the_tick() {
    let r = run("every (5)");
    // at t = 2 serial 1 holds 8 and the pool 12 of 10: nothing is taken
    // until the tick, which ends serial 0's lease
    assert_eq!(
        r.observe("free_after").unwrap().samples[0],
        -2.0,
        "{}",
        r.text()
    );
    assert_eq!(
        r.observe("again").unwrap().samples,
        vec![11.0],
        "{}",
        r.text()
    );
    assert_eq!(r.observe("let_in").unwrap().samples, vec![0.0, 0.0, 11.0]);
}

#[test]
fn a_lease_set_aside_comes_back_at_a_tick_before_its_lessee_asks() {
    let r = run("every (5) suspend");
    // set aside at t = 5, back at t = 10 once serial 1 has given its own
    // back: at t = 11 serial 0 still has its 4 and grows them
    assert!(
        r.observe("again").unwrap().samples.is_empty(),
        "{}",
        r.text()
    );
    assert_eq!(r.observe("let_in").unwrap().samples, vec![0.0, 0.0]);
    assert_eq!(
        r.observe("grown").unwrap().samples,
        vec![8.0, 8.0],
        "{}",
        r.text()
    );
    assert_eq!(r.ended, 2);
}

/// Without a tick, the growth itself takes the lease, as before.
#[test]
fn without_every_the_growth_takes_the_lease_at_once() {
    let r = run("");
    assert_eq!(
        r.observe("free_after").unwrap().samples[0],
        2.0,
        "{}",
        r.text()
    );
    assert_eq!(r.observe("again").unwrap().samples, vec![11.0]);
}

#[test]
fn a_tick_is_positive_and_suspend_needs_one_and_no_blocks() {
    let refused = |pool: &str, needle: &str| {
        let src = two("").replace("pool prog { cap 10; preempt leases by (serial) ; }", pool);
        let e = check_source(&common::main_source(&src), &common::horizon(10.0)).unwrap_err();
        assert!(e.contains(needle), "{e}");
    };
    refused(
        "pool prog { cap 10; preempt leases by (serial) every (0); }",
        "a tick is a positive, finite number of seconds",
    );
    refused(
        "pool prog { cap 10; preempt leases by (serial) suspend; }",
        "`suspend` without `every`",
    );
    refused(
        "pool prog { cap 10; block 2; preempt leases by (serial) every (5) suspend; }",
        "`suspend` on a pool with `block`",
    );
}
