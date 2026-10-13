//! `shrink P (n)`: the inverse of `grow`, a hold or a lease giving back
//! part of what it has. ThunderAgent counts a program calling a tool at
//! `acting_token_weight` this way (`examples/multi-turn/thunderagent.sq`).

mod common;

use serq::{check_source, run_source};

/// Serial 0 leases 8 of 10 units and gives 6 back at t = 1; serial 1,
/// which asks for 6, is let in then, not when serial 0 ends at t = 3.
#[test]
fn a_shrunk_lease_lets_a_waiting_hold_in() {
    let src = r#"
        stage svc : fifo(2);
        pool prog { cap 10; }
        workload { arrive batch(2); session { turn; } }
        server {
          if (serial == 0) {
            hold prog (cost(prog, 8)) { } lease prog (inf);
            run svc (cost(svc, 1));
            shrink prog (cost(prog, 6));
            observe left = allocated(prog);
            run svc (cost(svc, 2));
          } else {
            hold prog (cost(prog, 6)) { observe let_in = now; }
          }
        }
"#;
    let r = run_source(&common::main_source(src), &common::horizon(100.0), None).unwrap();
    assert_eq!(
        r.observe("left").unwrap().samples,
        vec![2.0],
        "{}",
        r.text()
    );
    assert_eq!(
        r.observe("let_in").unwrap().samples,
        vec![1.0],
        "{}",
        r.text()
    );
}

/// A hold gives back no more than it has not computed: of 10 units with 6
/// computed, `shrink (8)` leaves 6.
#[test]
fn a_hold_keeps_what_it_has_computed() {
    let src = r#"
        queue Q : decode {
          device gpu { kv cap 64; }
          engine on gpu { reqs cap 1; tokens cap 8; scheduler { advance running; admit waiting while (running.preempted == 0); } execute (1); }
          pool reqs on Q { }
          pool kv on Q.gpu { }
          decode (prompt) {
            hold kv (cost(kv, 10)), reqs (cost(reqs, 1)) {
              run Q prefill (cost(Q, 6)) growing kv;
              shrink kv (cost(kv, 8));
              observe left = allocated(kv);
            }
          }
        }
        workload { arrive batch(1); session { turn; } }
        server { Q.decode (6); }
"#;
    let r = run_source(&common::main_source(src), &common::horizon(100.0), None).unwrap();
    assert_eq!(
        r.observe("left").unwrap().samples,
        vec![6.0],
        "{}",
        r.text()
    );
}

#[test]
fn a_shrink_outside_every_hold_and_lease_does_not_link() {
    let src = r#"
        stage svc : fifo;
        pool prog { cap 10; }
        workload { arrive batch(1); session { turn; } }
        server { shrink prog (cost(prog, 4)); run svc (cost(svc, 1)); }
"#;
    let e = check_source(&common::main_source(src), &common::horizon(10.0)).unwrap_err();
    assert!(e.contains("outside a hold of `prog`"), "{e}");
}

/// A lease shrunk to nothing would read as one that has ended
/// (`allocated` 0) while it lasts on: the run fails and says `release`.
#[test]
fn a_lease_shrunk_to_nothing_fails_the_run() {
    let src = r#"
        stage svc : fifo;
        pool prog { cap 10; }
        workload { arrive batch(1); session { turn; } }
        server {
          hold prog (cost(prog, 4)) { } lease prog (inf);
          shrink prog (cost(prog, 4));
          run svc (cost(svc, 1));
        }
"#;
    let e = run_source(&common::main_source(src), &common::horizon(10.0), None).unwrap_err();
    assert!(e.contains("`release prog` ends it"), "{e}");
}

/// Under `reserve held` what a hold gives back is no longer reserved: a
/// hold reserving 8 and shrunk to 2 leaves 8 of 10 to the next, at t = 1.
#[test]
fn under_reserve_held_what_is_given_back_is_no_longer_reserved() {
    let src = r#"
        stage svc : fifo(2);
        pool prog { cap 10; reserve held; }
        workload { arrive batch(2); session { turn; } }
        server {
          if (serial == 0) {
            hold prog (cost(prog, 8)) reserve (cost(prog, 8)) {
              run svc (cost(svc, 1));
              shrink prog (cost(prog, 6));
              run svc (cost(svc, 2));
            }
          } else {
            run svc (cost(svc, 0.5));
            hold prog (cost(prog, 6)) { observe let_in = now; }
          }
        }
"#;
    let r = run_source(&common::main_source(src), &common::horizon(100.0), None).unwrap();
    assert_eq!(
        r.observe("let_in").unwrap().samples,
        vec![1.0],
        "{}",
        r.text()
    );
}
