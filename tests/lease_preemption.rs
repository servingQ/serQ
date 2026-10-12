//! A lease that a growth may take back (`docs/design/thunderagent.md`):
//! under `preempt leases by (keys)` a growth that does not fit ends the
//! lease with the least keys, and the lessee goes on wherever it is; `grow`
//! outside every hold of a pool grows the session's lease of it; and
//! `allocated(p)` is what the session has allocated there. ThunderAgent
//! pauses an idle program this way. `preempt by` still takes holds only.

mod common;

use serq::{check_source, run_source};

/// Two programs take 4 of 10 units each and lease them. Serial 1 comes
/// back first, at t = 2, and grows its lease by 4, which does not fit
/// beside serial 0's. Serial 0, at its tool until t = 11, then finds its
/// lease gone and is let in again when serial 1 has ended.
fn two(preempt: &str) -> String {
    format!(
        r#"
        stage svc : fifo(2);
        pool prog {{ cap 10; {preempt} }}
        workload {{ arrive batch(2);
          session {{ turn; tool (serial == 0 ? 10 : 1); turn; }}
        }}
        server {{
          branch (allocated(prog) > 0) {{
            grow prog (cost(prog, 4));
            observe grown = allocated(prog);
          }} else {{
            branch (turn_no > 1) {{ observe again = serial; }}
            hold prog (cost(prog, 4)) {{ observe let_in = now; }} lease prog (inf);
          }}
          run svc (cost(svc, 1));
        }}
"#
    )
}

#[test]
fn a_growth_takes_back_another_sessions_lease() {
    let r = run_source(
        &common::main_source(&two("preempt leases by (serial);")),
        &common::horizon(100.0),
        None,
    )
    .unwrap();
    assert_eq!(
        r.observe("grown").unwrap().samples,
        vec![8.0],
        "{}",
        r.text()
    );
    // serial 0's second request finds no lease, and waits for serial 1's end (t = 3)
    assert_eq!(
        r.observe("again").unwrap().samples,
        vec![0.0],
        "{}",
        r.text()
    );
    assert_eq!(r.observe("let_in").unwrap().samples, vec![0.0, 0.0, 11.0]);
    assert_eq!(r.ended, 2);
}

#[test]
fn the_grower_may_be_its_own_victim_and_goes_on_without_its_lease() {
    let r = run_source(
        &common::main_source(&two("preempt leases by (-serial);")),
        &common::horizon(100.0),
        None,
    )
    .unwrap();
    // serial 1's own lease is the least key: the growth ends it and nothing
    // is held; serial 0 kept its lease and grows it at t = 11
    assert_eq!(
        r.observe("grown").unwrap().samples,
        vec![0.0, 8.0],
        "{}",
        r.text()
    );
    assert!(r.observe("again").unwrap().samples.is_empty());
    assert_eq!(r.ended, 2);
}

/// Without `preempt leases by` both growths wait, each for room the other's lease
/// holds: the run says so rather than ending quietly.
#[test]
fn without_preempt_two_growing_leases_wait_for_each_other_loudly() {
    let r = run_source(
        &common::main_source(&two("")),
        &common::horizon(100.0),
        None,
    )
    .unwrap();
    assert!(r.observe("grown").unwrap().samples.is_empty());
    assert_eq!(r.ended, 0);
    assert!(
        r.text().contains("wait to grow in pool `prog`"),
        "{}",
        r.text()
    );
}

#[test]
fn a_grow_outside_every_hold_and_lease_does_not_link() {
    let src = r#"
        stage svc : fifo;
        pool prog { cap 10; }
        workload { arrive batch(1); session { turn; } }
        server { grow prog (cost(prog, 4)); run svc (cost(svc, 1)); }
"#;
    let e = check_source(&common::main_source(src), &common::horizon(10.0)).unwrap_err();
    assert!(e.contains("outside a hold of `prog`"), "{e}");
}

/// `preempt by` chooses among the holds in a scope, never a lease: serial
/// 1's growth finds no hold to preempt and waits, as without it.
#[test]
fn preempt_by_does_not_take_a_lease() {
    let r = run_source(
        &common::main_source(&two("preempt by (serial);")),
        &common::horizon(100.0),
        None,
    )
    .unwrap();
    assert!(r.observe("grown").unwrap().samples.is_empty());
    assert_eq!(r.pool("prog").unwrap().preemptions, 0);
}

/// The keys are a preempt key's: they may not draw.
#[test]
fn a_lease_key_may_not_draw() {
    let e = check_source(
        &common::main_source(&two("preempt leases by (~exp(1));")),
        &common::horizon(10.0),
    )
    .unwrap_err();
    assert!(e.contains("a preempt key may not draw"), "{e}");
}
