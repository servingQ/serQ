//! A step engine that serves by keys and preempts LIFO: the victim of a
//! growth, the latest admitted, need not be the last served, so it can be
//! one the iteration has already served. It leaves the batch, its tokens
//! return to the budget, its computed position stays where it was, and a
//! grower that preempted itself ends the iteration's serving (vLLM's
//! PRIORITY path, scheduler.py:779-813). The interpreter panicked costing
//! such an iteration.
//!
//! The scenario serves the newest first. Its answer is vLLM's at the pin
//! (0.30.1rc1.dev215+g0c87a197b) with the running loop visiting requests
//! newest first and the victim the latest admitted.

use serq::{Overrides, compile_source, program_path, run_ir};

const ENGINE: &str = "stage engine : step { budget B; chunk chunk; cost 1; memory kv; }";

#[test]
fn a_served_resident_preempted_in_its_iteration_leaves_it() {
    let src = std::fs::read_to_string(program_path("vllm_request")).unwrap();
    assert!(src.contains(ENGINE), "the oracle program's engine moved");
    // newest first; vLLM's chunk cap holds only with another request eligible
    let src = src.replace(
        ENGINE,
        "stage engine : step { budget B; chunk (residents + queued(reqs) > 1 ? chunk : 0); \
         cost 1; serve by (-admission); memory kv; }",
    );
    let mut ov = Overrides::default();
    for (k, v) in [
        ("bs", 16.0),
        ("B", 32.0),
        ("blocks", 9.0),
        ("max_seqs", 8.0),
        ("chunk", 24.0),
    ] {
        ov.set_num(k, v).unwrap();
    }
    let requests = [
        (60.0, 12.0, 1.0),
        (60.0, 24.0, 4.0),
        (60.0, 12.0, 0.0),
        (30.0, 6.0, 1.0),
    ];
    let sessions: Vec<Vec<(&str, f64)>> = requests
        .iter()
        .map(|&(p, o, a)| vec![("prompt", p), ("o", o), ("arrive", a)])
        .collect();
    let ir = compile_source(&src, &ov)
        .unwrap()
        .with_sessions(&sessions)
        .unwrap();
    let rep = run_ir(&ir, None).unwrap();
    let mut got = vec![(0.0, 0.0); requests.len()];
    for &(t, s, _) in &rep.observe("first").unwrap().records {
        got[s as usize].0 = t;
    }
    for &(t, s, _) in &rep.observe("done").unwrap().records {
        got[s as usize].1 = t;
    }
    assert_eq!(
        got,
        [(5.0, 26.0), (26.0, 49.0), (3.0, 16.0), (18.0, 23.0)],
        "{}",
        rep.text()
    );
    assert_eq!(rep.pool("kv").unwrap().preemptions, 2);
}
