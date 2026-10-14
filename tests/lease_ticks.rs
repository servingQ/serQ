//! A pool that acts at its ticks (`docs/design/thunderagent.md`): under
//! `preempt leases by (keys) every (dt)` a lease's growth is let through at
//! once, past the cap if it must, and at every multiple of `dt` the pool
//! ends leases in the keys' order while it is over its cap. With `suspend`
//! a lease the tick takes is set aside, and a later tick gives it back if
//! it fits, before the lessee asks again. ThunderAgent's scheduler loop:
//! `_pause_until_safe` at its tick, and `_greedy_resume` of a paused
//! program that has no request pending.

mod common;

/// Two programs lease 4 of 10 units. Serial 1 comes back at t = 2 and
/// grows its lease by 4: 12 of 10, let through until the tick at t = 5,
/// which takes serial 0's (the least key). Serial 1 gives its lease back
/// at t = 7. Serial 0 is at its tool until t = 11.
fn two(preempt: &str) -> String {
    format!(
        r#"
        stage svc : fifo(2);
        pool prog {{ cap 10; preempt leases by (serial) {preempt}; }}
        server (turn_no) {{
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

/// The workload of `two`: serial 0 at its tool for 10 s, serial 1 for 1.
const TWO: &str = "workload { arrive batch(2);
  send (turn_no);
  session { turn; tool (serial == 0 ? 10 : 1); turn; tool 4; turn; }
}";

fn run(preempt: &str) -> serq::Report {
    common::run(
        &common::main_source(&two(preempt)),
        TWO,
        &common::horizon(100.0),
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
    let r = run("every (5) suspend by (serial)");
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
        let e =
            common::compile(&common::main_source(&src), TWO, &common::horizon(10.0)).unwrap_err();
        assert!(e.contains(needle), "{e}");
    };
    refused(
        "pool prog { cap 10; preempt leases by (serial) every (0); }",
        "a tick is a positive, finite number of seconds",
    );
    refused(
        "pool prog { cap 10; preempt leases by (serial) suspend by (serial); }",
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
        pool prog { cap 10; preempt leases by (serial) every (5) suspend by (serial); }
        server (turn_no) {
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
    let workload = "workload { arrive batch(2);
      send (turn_no);
      session { turn; tool 1; turn; tool (serial == 0 ? 10 : 1); turn; tool 4; turn; }
    }";
    // the debug build's conservation check panicked at the tick, t = 5
    common::run(&common::main_source(src), workload, &common::horizon(100.0)).unwrap();
}

/// A hold that ends while its pool is over the cap, between ticks, cached
/// past the cap.
#[test]
fn a_pool_over_its_cap_between_ticks_caches_within_it() {
    let src = r#"
        stage svc : fifo(2);
        stage slow : fifo(1);
        pool prog { cap 10; preempt leases by (serial) every (5); }
        server (turn_no) {
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
    let workload = "workload { arrive batch(2);
      send (turn_no);
      session { turn; tool 1; turn; tool 10; turn; }
    }";
    common::run(&common::main_source(src), workload, &common::horizon(100.0)).unwrap();
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
            server (turn_no) {{
              if (serial == 0 && turn_no == 1) {{ hold prog (cost(prog, 4)) {{ }} lease prog (inf); }}
              if (serial == 1 && turn_no == 2) {{
                hold prog (cost(prog, 4)) {{ grow prog (cost(prog, 4)); }}
              }}
              if (serial == 0 && turn_no == 2) {{ observe back = allocated(prog); }}
              run svc (cost(svc, 1));
            }}
"#
        );
        let workload = "workload { arrive batch(2);
          send (turn_no);
          session { turn; tool (serial == 0 ? 10 : 2); turn; }
        }";
        common::run(
            &common::main_source(&src),
            workload,
            &common::horizon(100.0),
        )
        .unwrap()
        .observe("back")
        .unwrap()
        .samples[0]
    };
    assert_eq!(run("every (5) suspend by (serial)"), 4.0);
    assert_eq!(run("every (5)"), 0.0);
}

/// `suspend by` orders what comes back, apart from the victim's keys.
/// Three programs lease 3 of 10; at t = 2 serial 2 grows to 10, and the
/// tick at t = 5 sets aside serial 0's, then serial 1's (the least serial
/// first). At t = 7 serial 2 shrinks to 6: the tick at t = 10 has room for
/// one of the two, the first in the `suspend` keys' order. Serials 0 and 1
/// read their leases at t = 11.
#[test]
fn suspend_by_orders_the_leases_given_back() {
    let back = |keys: &str| {
        let src = format!(
            r#"
            stage svc : fifo(3);
            pool prog {{ cap 10; preempt leases by (serial) every (5) suspend by ({keys}); }}
            server (turn_no) {{
              if (turn_no == 1) {{ hold prog (cost(prog, 3)) {{ }} lease prog (inf); }}
              if (serial == 2 && turn_no == 2) {{ grow prog (cost(prog, 7)); }}
              if (serial == 2 && turn_no == 3) {{ shrink prog (cost(prog, 4)); }}
              if (serial < 2 && turn_no == 2) {{ observe back = allocated(prog); }}
              run svc (cost(svc, 1));
            }}
"#
        );
        let workload = "workload { arrive batch(3);
          send (turn_no);
          session { turn; tool (serial == 2 ? 1 : 10); turn; tool (serial == 2 ? 4 : 1); turn; tool 20; turn; }
        }";
        common::run(
            &common::main_source(&src),
            workload,
            &common::horizon(100.0),
        )
        .unwrap()
        .observe("back")
        .unwrap()
        .samples
        .clone()
    };
    // (serial 0's, serial 1's)
    assert_eq!(back("serial"), vec![3.0, 0.0]);
    assert_eq!(back("-serial"), vec![0.0, 3.0]);
}
