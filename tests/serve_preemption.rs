//! A step engine that serves by keys and preempts LIFO: the victim of a
//! growth, the latest admitted, need not be the last served, so it can be
//! one the iteration has already served. It leaves the batch, its tokens
//! return to the budget, its computed position stays where it was, and a
//! grower that preempted itself ends the iteration's serving (vLLM's
//! PRIORITY path, scheduler.py:779-813). The interpreter panicked costing
//! such an iteration.
//!
//! The scenario serves the newest first. Its answer is vLLM's at the pin
//! (0.30.1rc1.dev215+g0c87a197b) with the running loop patched to visit
//! requests newest first and to take the latest admitted as the victim:
//! stock vLLM has no such order (FCFS preempts `running[-1]`, PRIORITY the
//! largest `(priority, arrival_time)`). The patched scheduler,
//! `tools/serq_vllm.py`, is on the unpublished branch
//! `feat/serving-spec-language`, not in this repository. Run on the same
//! scheduler without the patch it gives the six oracle scenarios' answers.
//!
//! The scenario does not exercise the budget a served victim returns: the
//! answer is the same without it.

mod common;

use serq::{compile_source_at, program_path, run_ir};

const ENGINE: &str =
    "stage engine : step { budget B; chunk long_prefill(reqs, chunk); cost 1; memory kv; }";

#[test]
fn a_served_resident_preempted_in_its_iteration_leaves_it() {
    let path = program_path("vllm_request");
    let src = std::fs::read_to_string(&path).unwrap();
    assert!(src.contains(ENGINE), "the oracle program's engine moved");
    // newest first (the oracle program's chunk is vLLM's already: the cap
    // holds only with another request eligible)
    let src = src.replace(
        ENGINE,
        "stage engine : step { budget B; chunk long_prefill(reqs, chunk); \
         cost 1; serve by (-admission); memory kv; }",
    );
    let mut ov = common::example_options("vllm_request");
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
    let ir = compile_source_at(&common::main_source(&src), path.parent(), &ov)
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
