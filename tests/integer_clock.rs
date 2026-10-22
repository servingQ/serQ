//! The clock counts whole ticks (`docs/design/integer-clock.md`, IR 16):
//! every duration the run computes is rounded to the nearest tick, 1e-9 of
//! the clock's unit, where it is scheduled, so two sums of the same
//! durations are one instant; `waited`, `age` and `last` read the ticks
//! since. Before, the clock was an `f64` and "the same instant" was up to
//! the last bit of a sum: 0.2 + 0.1 came after 0.25 + 0.05, and 0.3 - 0.1
//! was under 0.2.

mod common;

use serq::Overrides;
use serq::ir::{TICKS_PER_UNIT, from_ticks, to_ticks};

/// `n` sessions arrive at t = 0, each one turn.
fn sessions(n: usize) -> String {
    format!("workload {{ arrive batch({n}); session {{ turn; }} }}\n")
}

fn run(n: usize, src: &str) -> serq::Report {
    common::run(
        &common::main_source(src),
        &sessions(n),
        &common::horizon(10.0),
    )
    .unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn fails(n: usize, src: &str, options: &Overrides) -> String {
    match common::run(&common::main_source(src), &sessions(n), options) {
        Err(e) => e,
        Ok(_) => panic!("ran:\n{src}"),
    }
}

fn samples(r: &serq::Report, name: &str) -> Vec<f64> {
    r.observe(name).unwrap().samples.clone()
}

// ---------------------------------------------------------- the rule ----

/// The nearest tick, a half to the even one: not a ceiling, which the last
/// bit of a product would move (1371 tokens at 2e-5 s is
/// 0.027420000000000003 in `f64`).
#[test]
fn a_duration_is_the_nearest_whole_tick() {
    assert_eq!(TICKS_PER_UNIT, 1e9);
    assert_eq!(to_ticks(1371.0 * 2e-5), 27_420_000);
    assert_eq!(to_ticks(0.2) + to_ticks(0.1), 300_000_000);
    assert_eq!(to_ticks(0.25) + to_ticks(0.05), 300_000_000);
    assert_ne!(0.2 + 0.1, 0.25 + 0.05);
    assert_eq!(to_ticks(4e-10), 0);
    assert_eq!(to_ticks(6e-10), 1);
    assert_eq!(to_ticks(0.5e-9), 0);
    assert_eq!(to_ticks(1.5e-9), 2);
    assert_eq!(to_ticks(f64::NAN), 0);
    assert_eq!(to_ticks(-1.0), 0);
    assert_eq!(to_ticks(f64::MAX), u64::MAX);
    assert_eq!(from_ticks(300_000_000), 0.3);
    assert_eq!(from_ticks(27_420_000), 0.02742);
}

// ---------------------------------------------- what the rule changes ----

/// Session 0 reaches the gate after runs of 0.2 then 0.1, session 1 after
/// 0.25 then 0.05: one instant, t = 0.3, where session 0's run was
/// scheduled first and is admitted first. Before, session 0 arrived at
/// 0.30000000000000004, after session 1, and was admitted at 1.3.
#[test]
fn two_sums_of_one_time_are_one_instant() {
    let r = run(
        2,
        r#"
        stage d : delay;
        pool gate { cap 1; }
        server {
          if (serial == 0) { run d (cost(d, 0.2)); run d (cost(d, 0.1)); } else { run d (cost(d, 0.25)); run d (cost(d, 0.05)); }
          hold gate (cost(gate, 1)) { observe who = serial; observe at = now; run d (cost(d, 1)); }
        }
"#,
    );
    assert_eq!(samples(&r, "who"), [0.0, 1.0]);
    assert_eq!(samples(&r, "at"), [0.3, 1.3]);
}

/// `waited` is the ticks since the hold queued. Session 0 holds the gate
/// until 0.25 + 0.05; session 1 queued at 0.1, session 2 at 0.15. At 0.3
/// session 1 has waited 0.2 and goes first. Before, it had waited
/// 0.3 - 0.1 = 0.19999999999999998, and the newest serial went first.
#[test]
fn waited_is_the_ticks_since_the_hold_queued() {
    let r = run(
        3,
        r#"
        stage d : delay;
        pool gate { cap 1; queue by (waited >= 0.2 ? 0 : 1, -serial); }
        server {
          if (serial == 1) { run d (cost(d, 0.1)); }
          if (serial == 2) { run d (cost(d, 0.15)); }
          hold gate (cost(gate, 1)) {
            observe who = serial;
            observe at = now;
            if (serial == 0) { run d (cost(d, 0.25)); run d (cost(d, 0.05)); } else { run d (cost(d, 1)); }
          }
        }
"#,
    );
    assert_eq!(samples(&r, "who"), [0.0, 1.0, 2.0]);
    assert_eq!(samples(&r, "at"), [0.0, 0.3, 1.3]);
}

/// `age` is the ticks since the entry's release. Session 0 releases 10
/// cached units at 0.1, session 1 releases 5 at 0.2; at 0.3 session 2
/// needs 15 of 25 and one entry goes, the old ones (`age >= 0.2`) first.
/// Session 0's is 0.2 old and goes; before, it was 0.19999999999999998
/// old, both entries were young, and the smaller, session 1's, went.
#[test]
fn age_is_the_ticks_since_the_release() {
    let r = run(
        3,
        r#"
        pool kv { cap 25; evict by (age >= 0.2 ? 0 : 1, size); }
        stage d : delay;
        server {
          if (serial == 0) { hold kv (cost(kv, 10)) { run d (cost(d, 0.1)); } cache (cost(kv, 10)); }
          if (serial == 1) { run d (cost(d, 0.15)); hold kv (cost(kv, 5)) { run d (cost(d, 0.05)); } cache (cost(kv, 5)); }
          if (serial == 2) { run d (cost(d, 0.3)); hold kv (cost(kv, 15)) { run d (cost(d, 0.1)); } }
          run d (cost(d, 1 + serial));
          if (serial == 0) { hold kv (cost(kv, 10)) { observe kept0 = cached; } cache (cost(kv, 10)); }
          if (serial == 1) { hold kv (cost(kv, 5)) { observe kept1 = cached; } cache (cost(kv, 5)); }
        }
"#,
    );
    assert_eq!(samples(&r, "kept0"), [0.0]);
    assert_eq!(samples(&r, "kept1"), [5.0]);
}

/// A duration under half a tick takes none, and one over it takes one:
/// 4e-10 ends at t = 0, a further 6e-10 at 1e-9. Before, the first ended
/// at 4e-10.
#[test]
fn a_duration_under_half_a_tick_takes_none() {
    let r = run(
        1,
        r#"
        stage d : delay;
        server { run d (cost(d, 4e-10)); observe a = now; run d (cost(d, 6e-10)); observe b = now; }
"#,
    );
    assert_eq!(samples(&r, "a"), [0.0]);
    assert_eq!(samples(&r, "b"), [1e-9]);
}

/// pd_batching's baseline copy is free (`Bw` 1e15 tokens/s): its read ends
/// at the instant of the decoder's admission, so `D.load_done -
/// D.admitted` is 0. Before, it was about 2e-12 with a cv2 of 0.09, the
/// noise of `f64` sums.
#[test]
fn a_free_copy_takes_no_time() {
    let ov = Overrides {
        horizon: Some(20.0),
        seed: Some(1),
        ..Default::default()
    };
    let r = common::run_example("examples/pd-disaggregation/pd_batching.sq", &ov).unwrap();
    let transfer = r.observe("transfer").unwrap();
    assert!(transfer.count > 1000, "{}", transfer.count);
    assert!(transfer.samples.iter().all(|&t| t == 0.0));
}

// ------------------------ what a fixed resolution cannot run, refused ----

/// A pool's `every` under half a tick would tick without time passing.
/// Before, it ran; the integer clock without this check divides by zero.
#[test]
fn a_lease_tick_under_a_tick_does_not_link() {
    let e = common::compile(
        &common::main_source(
            r#"
        stage svc : fifo(2);
        pool prog { cap 10; preempt leases by (serial) every (1e-10); }
        server (turn_no) {
          if (turn_no == 2) { release prog; end; }
          hold prog (cost(prog, 4)) { run svc (cost(svc, 1)); } lease prog (inf);
        }
"#,
        ),
        "workload { arrive batch(2); send (turn_no); session { turn; turn; } }",
        &common::horizon(10.0),
    )
    .unwrap_err();
    assert!(
        e.contains("`every (0.0000000001)` is under half a tick of the clock (1e-9 of its unit)"),
        "{e}"
    );
}

/// The same for a queue that admits at its ticks.
#[test]
fn a_queue_tick_under_a_tick_does_not_link() {
    let e = common::compile(
        &common::main_source(
            r#"
        stage svc : fifo(1);
        pool gate { queue fifo every (1e-10); }
        server { hold gate (cost(gate, 1)) { run svc (cost(svc, 1)); } }
"#,
        ),
        &sessions(1),
        &common::horizon(10.0),
    )
    .unwrap_err();
    assert!(
        e.contains("`queue … every (0.0000000001)` is under half a tick of the clock"),
        "{e}"
    );
}

/// A constant interarrival time under half a tick would bring every
/// arrival at t = 0. Before, the run did not end.
#[test]
fn a_constant_renewal_gap_under_a_tick_does_not_link() {
    let e = common::compile(
        &common::main_source("stage d : delay;\nserver { run d (cost(d, 1)); }"),
        "workload { arrive renewal(1e-10); session { turn; } }",
        &common::horizon(10.0),
    )
    .unwrap_err();
    assert!(
        e.contains("an interarrival time is at least one tick of the clock (1e-9 of its unit)"),
        "{e}"
    );
}

/// A warmup and a horizon on one tick measure nothing.
#[test]
fn a_warmup_on_the_horizons_tick_does_not_run() {
    let e = fails(
        1,
        "stage d : delay;\nserver { run d (cost(d, 1)); }",
        &common::horizon(1e-10),
    );
    assert!(
        e.contains("warmup 0 and horizon 0.0000000001 round to one tick of the clock"),
        "{e}"
    );
}

/// The clock is a `u64` of ticks: a horizon past its last tick, about
/// 1.8e10 of its unit, is an error, not a clock that stops there.
#[test]
fn a_horizon_past_the_clocks_end_is_an_error() {
    let e = fails(
        1,
        "stage d : delay;\nserver { run d (cost(d, 1)); }",
        &common::horizon(1e11),
    );
    assert!(
        e.contains("horizon 100000000000 is past the clock's last tick"),
        "{e}"
    );
}

/// An iteration that schedules tokens lasts at least one tick: a `forward`
/// of 1e-10 is as a `forward` of 0. Before, it ran.
#[test]
fn an_iteration_under_half_a_tick_is_an_error() {
    let e = fails(
        1,
        r#"
        device gpu { kv cap 1000; }
        engine llm on gpu {
          tokens cap 512;
          scheduler { advance running; admit waiting while (running.preempted == 0) taking kv (100); }
          worker { forward (1e-10); }
        }
        pool kv on gpu { block 16; evict lru; }
        server { llm (100, 1); }
"#,
        &common::horizon(10.0),
    );
    assert!(
        e.contains(
            "its `worker` is 0.0000000001 for an iteration of 100 tokens, under half a tick"
        ),
        "{e}"
    );
}

/// Drawn gaps under half a tick bring the arrivals at one instant, without
/// end; the run stops at as many as a `batch` may hold. Before, the clock
/// crept on by the gaps and the run did not end.
#[test]
fn a_flood_of_arrivals_at_one_instant_is_an_error() {
    let e = common::run(
        &common::main_source("stage d : delay;\nserver { run d (cost(d, 1)); }"),
        "workload { arrive renewal(~uniform(1e-11, 1e-10)); session { turn; } }",
        &common::horizon(10.0),
    )
    .map(|_| ())
    .unwrap_err();
    assert!(
        e.contains("1000001 arrivals at the instant t = 0: its gaps are under half a tick"),
        "{e}"
    );
}

/// A `run` under half a tick in a loop is a run of zero work: the instant
/// does not settle, and the message says why it might not.
#[test]
fn a_run_under_half_a_tick_in_a_loop_does_not_settle() {
    let e = fails(
        1,
        "stage d : delay;\nserver { loop { run d (cost(d, 1e-10)); } }",
        &common::horizon(10.0),
    );
    assert!(e.contains("the instant t = 0 does not settle"), "{e}");
    assert!(
        e.contains("of under half a tick of the clock (1e-9 of its unit)"),
        "{e}"
    );
}
