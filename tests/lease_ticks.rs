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
fn a_tick_is_positive_and_suspend_needs_one_and_reserve_held_none() {
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
        "pool prog { cap 10; reserve held; preempt leases by (serial) every (5); }",
        "`every` on a pool with `reserve held`",
    );
}

/// Found in review (#591), each a debug-build panic and a silent miscount
/// in release. A lease set aside let go of the cached prefix it read: the
/// trie kept its lock, and its end, which returns early for a set-aside
/// lease, never unlocked it.
#[test]
fn a_lease_set_aside_lets_go_of_the_prefix_it_read() {
    let src = r#"
        stage svc : fifo(2);
        pool prog { cap 10; preempt leases by (serial) every (5) suspend; }
        workload { arrive batch(2);
          session { turn; tool 1; turn; tool (serial == 0 ? 10 : 1); turn; tool 4; turn; }
        }
        server {
          if (serial == 1 && turn_no == 4) { release prog; end; }
          if (serial == 0 && turn_no == 1) {
            hold prog (cost(prog, 4)) { run svc (cost(svc, 1)); } cache (cost(prog, 4));
          }
          if (allocated(prog) > 0) {
            if (serial == 1 && turn_no == 3) { grow prog (cost(prog, 4)); }
          } else {
            if (serial == 1 || turn_no == 2) {
              hold prog (cost(prog, 4)) { observe c = cached; } cache (cost(prog, 4)) lease prog (inf);
            }
          }
          run svc (cost(svc, 1));
        }
"#;
    // the debug build's conservation check panicked at the tick, t = 5
    run_source(&common::main_source(src), &common::horizon(100.0), None).unwrap();
}

/// A hold that ends while its pool is over the cap, between ticks, cached
/// past the cap.
#[test]
fn a_pool_over_its_cap_between_ticks_caches_within_it() {
    let src = r#"
        stage svc : fifo(2);
        stage slow : fifo(1);
        pool prog { cap 10; preempt leases by (serial) every (5); }
        workload { arrive batch(2);
          session { turn; tool 1; turn; tool 10; turn; }
        }
        server {
          if (serial == 0 && turn_no == 1) {
            hold prog (cost(prog, 4)) { run slow (cost(slow, 3)); } cache (cost(prog, 4));
          }
          if (serial == 1) {
            if (allocated(prog) > 0) {
              if (turn_no == 2) { grow prog (cost(prog, 4)); }
            } else {
              hold prog (cost(prog, 4)) { } lease prog (inf);
            }
          }
          run svc (cost(svc, 1));
        }
"#;
    // serial 1's lease grows to 8 at t = 2 and the pool holds 12 of 10;
    // serial 0's hold ends at t = 3 and cached 4 past the cap
    run_source(&common::main_source(src), &common::horizon(100.0), None).unwrap();
}

/// A hold's growth is an admission's and needs room at once, `every` or
/// not; under `suspend` the lease it takes is set aside, as a tick's is,
/// and a later tick gives it back. Serial 0 leases 4 of 10; at t = 3
/// serial 1's hold grows to 8, which takes serial 0's lease, and ends. The
/// tick at t = 5 gives the lease back; serial 0 reads it at t = 11.
#[test]
fn a_holds_growth_sets_a_lease_aside_as_a_tick_does() {
    let run = |preempt: &str| {
        let src = format!(
            r#"
            stage svc : fifo(2);
            pool prog {{ cap 10; preempt leases by (serial) {preempt}; }}
            workload {{ arrive batch(2);
              session {{ turn; tool (serial == 0 ? 10 : 2); turn; }}
            }}
            server {{
              if (serial == 0 && turn_no == 1) {{ hold prog (cost(prog, 4)) {{ }} lease prog (inf); }}
              if (serial == 1 && turn_no == 2) {{
                hold prog (cost(prog, 4)) {{ grow prog (cost(prog, 4)); }}
              }}
              if (serial == 0 && turn_no == 2) {{ observe back = allocated(prog); }}
              run svc (cost(svc, 1));
            }}
"#
        );
        run_source(&common::main_source(&src), &common::horizon(100.0), None)
            .unwrap()
            .observe("back")
            .unwrap()
            .samples[0]
    };
    assert_eq!(run("every (5) suspend"), 4.0);
    assert_eq!(run("every (5)"), 0.0);
}
