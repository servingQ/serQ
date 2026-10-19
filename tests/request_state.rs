//! A request's state (`serq::engine::state`, `docs/design/request-state.md`):
//! where a session or a leg stands while time passes, at about the
//! resolution of vLLM's `RequestStatus`, in the run log and the report.

mod common;

use serq::engine::log::{EventKind, Log, Who};
use serq::engine::state::State;
use serq::frontend::ast::RunMode;
use serq::{run_ir, run_ir_logged};
use std::path::{Path, PathBuf};

/// The example at `path` with its workload (`examples/pairs.json`), and the
/// directory its trace is read from.
fn load(path: &Path, ov: serq::Overrides) -> (serq::Program, PathBuf) {
    let w = serq::paired_workload(path).expect("a paired workload");
    let p = serq::load_paired(path, &w, &ov).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    (p, w.parent().unwrap().to_path_buf())
}

fn logged(name: &str, horizon: f64) -> serq::Report {
    let (p, base) = load(&serq::program_path(name), common::horizon(horizon));
    run_ir_logged(&p, Some(&base), usize::MAX).unwrap()
}

const SESSION: Who = Who {
    serial: 0,
    leg: None,
};

/// The states `who` entered, in order, an engine resident's with its mode.
fn states(log: &Log, who: Who) -> Vec<String> {
    log.events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::State {
                who: w,
                state,
                mode,
            } if *w == who => Some(match mode {
                RunMode::Plain => state.name().to_string(),
                RunMode::Prefill => format!("{} prefill", state.name()),
                RunMode::Decode => format!("{} decode", state.name()),
            }),
            _ => None,
        })
        .collect()
}

fn state(r: &serq::Report, s: State) -> &serq::engine::report::StateReport {
    r.states.iter().find(|x| x.name == s.name()).unwrap()
}

/// `preempt.json`, the vLLM oracle's scenario: two requests prefill at
/// t = 0; at t = 1 the second's growth does not fit and it is preempted
/// (`_preempt_request`, scheduler.py:1560 sets `PREEMPTED`), waits until the
/// first finishes at t = 30, and runs again: preempted for 29 s, its prefill
/// and decode one stay of running each time. vLLM keeps `PREEMPTED` until
/// it is scheduled (scheduler.py:1306, 1319).
#[test]
fn a_preempted_request_is_preempted_until_it_runs_again() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/oracle");
    let p: serq::Program =
        serde_json::from_str(&std::fs::read_to_string(dir.join("preempt.ir.json")).unwrap())
            .unwrap();
    let r = run_ir_logged(&p, None, usize::MAX).unwrap();
    let log = r.log.as_ref().unwrap();
    let second = Who {
        serial: 1,
        leg: None,
    };
    assert_eq!(
        states(log, second),
        [
            "waiting",
            "running prefill",
            "preempted",
            "running prefill",
            "running decode"
        ]
    );
    assert_eq!(
        states(log, SESSION),
        ["waiting", "running prefill", "running decode"]
    );
    let pre = state(&r, State::Preempted);
    assert_eq!((pre.stays, pre.mean), (1, 29.0), "{}", r.text());
    // the first's prefill and decode are one stay; the second's two
    assert_eq!(state(&r, State::Running).stays, 3);
    assert!(r.text().contains("\nstate "), "{}", r.text());
}

/// `llmd_nixl_pull.sq`'s first session: the prefiller's hold waits and
/// prefills; the decoder's request is admitted to its blocks, and its
/// connector's load, the read over both NICs, is `loading` (vLLM's
/// `WAITING_FOR_REMOTE_KVS`, scheduler.py:1267); with its KV it waits again
/// for a running slot (`WAITING` again, scheduler.py:3088-3091), and runs.
/// The prefiller's blocks are leased from its prefill's end until the read
/// is done: vLLM's finished request whose free is delayed
/// (scheduler.py:2653-2655), a `lease` and a `free`, not a state.
#[test]
fn a_connector_load_is_loading_and_the_prefillers_blocks_are_leased() {
    let r = logged("llmd_nixl_pull", 30.0);
    let log = r.log.as_ref().unwrap();
    assert_eq!(
        &states(log, SESSION)[..7],
        [
            "waiting",
            "running prefill",
            "waiting",
            "loading",
            "waiting",
            "running prefill",
            "running decode"
        ]
    );
    let at = |f: &dyn Fn(&EventKind) -> bool| log.events.iter().position(|e| f(&e.kind)).unwrap();
    let lease = at(&|k| matches!(k, EventKind::Lease { who, .. } if *who == SESSION));
    let EventKind::Lease { id, pool, .. } = log.events[lease].kind else {
        unreachable!()
    };
    assert_eq!(log.pools[pool].0, "P.kv");
    let loading = at(
        &|k| matches!(k, EventKind::State { who, state: State::Loading, .. } if *who == SESSION),
    );
    let free = at(&|k| matches!(k, EventKind::Free { id: i, .. } if *i == id));
    assert!(lease < loading && loading < free);
    // freed when the read is done, at the end of its loading
    let done = log.events[loading..]
        .iter()
        .find(|e| matches!(e.kind, EventKind::Done { who, .. } if who == SESSION))
        .unwrap();
    assert_eq!(log.events[free].t, done.t);
}

/// `vllm_nixl_push.sq`: the prefill is a leg (`fork`), which waits and
/// prefills; its blocks are leased when its prefill ends, and it ends
/// (`finished`) with the lease passed to its session, which frees it when
/// the push is done. The decoder's request waits for the push inside its
/// connector's load, a `join` of the leg: `loading`, not `joining`.
#[test]
fn a_leg_finishes_with_its_blocks_leased() {
    let r = logged("vllm_nixl_push", 5.0);
    let log = r.log.as_ref().unwrap();
    let leg = Who {
        serial: 0,
        leg: Some(0),
    };
    assert_eq!(states(log, leg), ["waiting", "running prefill"]);
    let s = states(log, SESSION);
    assert_eq!(&s[..3], ["running", "waiting", "loading"], "{s:?}");
    assert!(!s.contains(&"joining".to_string()), "{s:?}");
    let pos = |f: &dyn Fn(&EventKind) -> bool| log.events.iter().position(|e| f(&e.kind)).unwrap();
    let lease = pos(&|k| matches!(k, EventKind::Lease { who, .. } if *who == leg));
    let EventKind::Lease { id, .. } = log.events[lease].kind else {
        unreachable!()
    };
    let end = pos(&|k| matches!(k, EventKind::End { who } if *who == leg));
    let free = pos(&|k| matches!(k, EventKind::Free { id: i, .. } if *i == id));
    assert!(lease < end && end < free);
    // the session holds it when it is freed
    assert!(matches!(log.events[free].kind, EventKind::Free { who, .. } if who == SESSION));
    assert!(log.events[free].t > log.events[end].t);
}

/// `pd_two_fabrics.sq`: the KV read over a prefiller's port and a
/// decoder's at once, outside any connector, is `transferring`.
#[test]
fn a_transfer_over_two_stages_is_transferring() {
    let r = logged("pd_two_fabrics", 5.0);
    let log = r.log.as_ref().unwrap();
    assert_eq!(
        states(log, SESSION),
        ["waiting", "running", "waiting", "transferring", "running"]
    );
    assert!(state(&r, State::Transferring).stays > 0);
}

/// `vllm_lmcache.sq`: the connector's save, before the request's blocks go
/// back, is `saving`; a turn whose prompt the L2 holds loads it first.
#[test]
fn a_connector_save_is_saving() {
    let r = logged("vllm_lmcache", 60.0);
    let log = r.log.as_ref().unwrap();
    assert_eq!(
        states(log, SESSION)[..4],
        ["waiting", "running prefill", "running decode", "saving"]
    );
    assert!(state(&r, State::Saving).stays > 0);
    assert!(state(&r, State::Loading).stays > 0);
}

/// The closed M/M/1//N of `closed.sq`: a session waits in the FIFO stage's
/// queue (`waiting`), is served (`running`) and thinks at its user station,
/// a run the workload issues (`client`). The states hold every session:
/// the ones waiting and served are the stage's, the one served its
/// utilization.
#[test]
fn a_fifo_queue_is_waiting_and_a_think_time_is_the_client() {
    let r = common::run_example("examples/multi-turn/closed.sq", &common::horizon(2000.0)).unwrap();
    let (w, run, c) = (
        state(&r, State::Waiting).number,
        state(&r, State::Running).number,
        state(&r, State::Client).number,
    );
    let svc = r.stage("svc").unwrap();
    let close = |a: f64, b: f64| (a - b).abs() < 1e-9 * b.abs().max(1.0);
    assert!(close(run, svc.utilization), "{run} {}", svc.utilization);
    assert!(
        close(w + run, svc.mean_number),
        "{w} {run} {}",
        svc.mean_number
    );
    assert!(close(w + run + c, r.mean_live), "{}", r.text());
    assert!(close(r.mean_live, 8.0));
}

/// A `grow` that does not fit under `preempt none` waits for room
/// (`growing`), here 1 s; a session at a `join` waits for its leg
/// (`joining`), here 5 - 3 = 2 s.
#[test]
fn a_growth_that_waits_is_growing_and_a_join_is_joining() {
    let grow = r#"
        pool kv { cap 100; preempt none; }
        stage svc : fifo(2);
        stage gate : delay;
        server {
          run gate (cost(gate, serial));
          hold kv (cost(kv, 50)) {
            run svc (cost(svc, 5));
            if (serial == 0) { grow kv (cost(kv, 30)); }
          }
        }
"#;
    let two = "workload { arrive batch(2); session { turn; end; } }";
    let r = common::run(&common::main_source(grow), two, &common::horizon(100.0)).unwrap();
    let g = state(&r, State::Growing);
    assert_eq!((g.stays, g.mean), (1, 1.0), "{}", r.text());

    let join = r#"
        stage svc : delay;
        server {
          fork { run svc (cost(svc, 5)); }
          run svc (cost(svc, 3));
          join;
        }
"#;
    let one = "workload { arrive batch(1); session { turn; end; } }";
    let p = common::compile(&common::main_source(join), one, &common::horizon(100.0)).unwrap();
    let r = run_ir_logged(&p, None, usize::MAX).unwrap();
    let j = state(&r, State::Joining);
    assert_eq!((j.stays, j.mean), (1, 2.0), "{}", r.text());
    let log = r.log.as_ref().unwrap();
    assert_eq!(states(log, SESSION), ["running", "joining"]);
    let leg = Who {
        serial: 0,
        leg: Some(0),
    };
    assert_eq!(states(log, leg), ["running"]);
}

/// In every example program a session or a leg that is live is in one
/// state, save for no time while it is ready: the time-average numbers in
/// the states add up to the live sessions, and to more when legs run
/// beside them. The log's state events are the report's stays, and two in
/// a row about one session or leg differ.
#[test]
fn the_states_add_up_to_the_live_sessions() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut n = 0;
    for group in std::fs::read_dir(&root).unwrap().flatten() {
        if !group.path().is_dir() || group.file_name() == "workloads" {
            continue;
        }
        for entry in std::fs::read_dir(group.path()).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "sq") {
                continue;
            }
            let (p, base) = load(&path, common::horizon(20.0));
            let r = run_ir_logged(&p, Some(&base), usize::MAX).unwrap();
            let log = r.log.as_ref().unwrap();
            let total: f64 = r.states.iter().map(|x| x.number).sum();
            let forks = log
                .events
                .iter()
                .any(|e| matches!(e.kind, EventKind::Fork { .. }));
            let eps = 1e-9 * r.mean_live.max(1.0);
            if forks {
                assert!(total >= r.mean_live - eps, "{}", path.display());
            } else {
                assert!(
                    (total - r.mean_live).abs() <= eps,
                    "{}: {total} != {}",
                    path.display(),
                    r.mean_live
                );
            }
            let mut last: std::collections::HashMap<(u64, Option<u32>), (State, RunMode)> =
                Default::default();
            for e in &log.events {
                if let EventKind::State { who, state, mode } = e.kind {
                    let prev = last.insert((who.serial, who.leg), (state, mode));
                    assert_ne!(prev, Some((state, mode)), "{}", path.display());
                }
                if let EventKind::End { who } = e.kind {
                    last.remove(&(who.serial, who.leg));
                }
            }
            // the report is the same without the log
            let plain = run_ir(&p, Some(&base)).unwrap();
            for (a, b) in plain.states.iter().zip(&r.states) {
                assert_eq!(a.stays, b.stays, "{}", path.display());
            }
            n += 1;
        }
    }
    assert!(n >= 30, "{n} programs");
}
