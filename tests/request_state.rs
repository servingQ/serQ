//! An engine request's status and its transfers (`serq::engine::state`,
//! `docs/design/request-state.md`): one status per request at any time,
//! vLLM's `RequestStatus`, and the KV moves of it beside the status, in the
//! run log and the report.

mod common;

use serq::engine::log::{EventKind, Log, Who};
use serq::engine::state::{Moved, Status, Transfer};
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

/// The statuses request `call` of `who` entered, in order, with its mode.
fn statuses(log: &Log, who: Who, call: u32) -> Vec<String> {
    log.events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Status {
                who: w,
                call: c,
                status,
                mode,
                ..
            } if *w == who && *c == call => Some(match mode {
                RunMode::Plain => status.name().to_string(),
                RunMode::Prefill => format!("{} prefill", status.name()),
                RunMode::Decode => format!("{} decode", status.name()),
            }),
            _ => None,
        })
        .collect()
}

/// The transfer events of request `call` of `who`: (kind, moved, t).
fn transfers(log: &Log, who: Who, call: u32) -> Vec<(Transfer, Moved, f64)> {
    log.events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Transfer {
                who: w,
                call: c,
                kind,
                moved,
                ..
            } if *w == who && *c == call => Some((*kind, *moved, e.t)),
            _ => None,
        })
        .collect()
}

fn status(r: &serq::Report, s: Status) -> &serq::engine::report::StatusReport {
    r.statuses.iter().find(|x| x.name == s.name()).unwrap()
}

/// `preempt.json`, the vLLM oracle's scenario: two requests prefill at
/// t = 0; at t = 1 the second's growth does not fit and it is preempted
/// (`_preempt_request`, scheduler.py:1560 sets `PREEMPTED`), waits until the
/// first finishes at t = 30, and runs again (scheduler.py:1306, 1319):
/// preempted for 29 s. Its prefill and decode are one stay of running each
/// time.
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
        statuses(log, second, 1),
        [
            "waiting",
            "running prefill",
            "preempted",
            "running prefill",
            "running decode",
            "finished"
        ]
    );
    assert_eq!(
        statuses(log, SESSION, 1),
        ["waiting", "running prefill", "running decode", "finished"]
    );
    let pre = status(&r, Status::Preempted);
    assert_eq!((pre.stays, pre.mean), (1, 29.0), "{}", r.text());
    assert_eq!(status(&r, Status::Running).stays, 3);
    // a request that frees its blocks at its end is not counted finished
    assert_eq!(status(&r, Status::Finished).stays, 0);
    assert!(r.text().contains("\nstatus "), "{}", r.text());
}

/// `llmd_nixl_pull.sq`'s first session, a remote prefill: the prefiller's
/// request (call 1) runs and finishes with a send registered, its blocks
/// kept (vLLM's `delay_free_blocks`, scheduler.py:2653-2655); the decoder's
/// (call 2) is admitted to its blocks and waits for its KV while its load
/// reads the send (`WAITING_FOR_REMOTE_KVS`, scheduler.py:1267), waits for a
/// slot (`WAITING` again, scheduler.py:3088-3091), and runs. The send is
/// done when the load is (`finished_sending`, scheduler.py:3135-3138): the
/// prefiller's request is finished from its end to then.
#[test]
fn a_read_is_waiting_for_kv_and_the_prefiller_s_send_keeps_it_finished() {
    let r = logged("llmd_nixl_pull", 30.0);
    let log = r.log.as_ref().unwrap();
    assert_eq!(
        statuses(log, SESSION, 1),
        ["waiting", "running prefill", "finished"]
    );
    assert_eq!(
        statuses(log, SESSION, 2),
        [
            "waiting",
            "waiting_for_kv",
            "waiting",
            "running prefill",
            "running decode",
            "finished"
        ]
    );
    let send = transfers(log, SESSION, 1);
    let load = transfers(log, SESSION, 2);
    let kinds = |v: &[(Transfer, Moved, f64)]| v.iter().map(|x| (x.0, x.1)).collect::<Vec<_>>();
    assert_eq!(
        kinds(&send),
        [
            (Transfer::Send, Moved::Registered),
            (Transfer::Send, Moved::Start),
            (Transfer::Send, Moved::Done)
        ]
    );
    assert_eq!(
        kinds(&load),
        [
            (Transfer::Load, Moved::Registered),
            (Transfer::Load, Moved::Start),
            (Transfer::Load, Moved::Done)
        ]
    );
    // one copy: the send starts and ends with the load
    assert_eq!((send[1].2, send[2].2), (load[1].2, load[2].2));
    assert!(send[0].2 <= load[0].2);
    // the prefiller's request is finished while its send holds its blocks:
    // the program's own `lease` observation (taken after the decoder's call
    // returns) is that time, sample for sample
    let mut held: Vec<f64> = vec![];
    let mut since = std::collections::HashMap::new();
    for e in &log.events {
        if let EventKind::Transfer {
            who,
            call,
            kind: Transfer::Send,
            moved,
            ..
        } = &e.kind
        {
            match moved {
                Moved::Registered => {
                    since.insert((who.serial, who.leg, *call), e.t);
                }
                Moved::Done => held.push(e.t - since[&(who.serial, who.leg, *call)]),
                Moved::Start => {}
            }
        }
    }
    let lease = r.observe("send_held").unwrap();
    assert!(lease.count > 0);
    assert_eq!(status(&r, Status::Finished).stays as usize, held.len());
    for x in &lease.samples {
        assert!(held.iter().any(|h| (h - x).abs() < 1e-12), "{x} {held:?}");
    }
}

/// `vllm_nixl_push.sq`: the prefill is a leg's request, the decoder's is the
/// session's, at once. The decoder's request waits for KV until the leg's
/// prefill has finished and the push is done; nothing joins. Each request
/// has one status at a time.
#[test]
fn a_push_s_reader_waits_for_kv_until_the_leg_s_send() {
    let r = logged("vllm_nixl_push", 5.0);
    let log = r.log.as_ref().unwrap();
    let leg = Who {
        serial: 0,
        leg: Some(0),
    };
    assert_eq!(
        statuses(log, leg, 1),
        ["waiting", "running prefill", "finished"]
    );
    let d = statuses(log, SESSION, 1);
    assert_eq!(&d[..3], ["waiting", "waiting_for_kv", "waiting"], "{d:?}");
    let registered = transfers(log, leg, 1)[0];
    assert_eq!(
        (registered.0, registered.1),
        (Transfer::Send, Moved::Registered)
    );
    let load = transfers(log, SESSION, 1);
    // the decoder waited for KV from before the send existed
    assert!(load[0].2 < registered.2, "{load:?} {registered:?}");
    assert_eq!(
        load[1].2, registered.2,
        "the copy starts when the send is there"
    );
}

/// `vllm_lmcache.sq`: the connector's save runs after the request has
/// generated its output, its blocks kept until the copy is done: finished,
/// with a save. A turn whose prompt the L2 holds waits for KV while it
/// loads.
#[test]
fn a_save_keeps_its_request_finished() {
    let r = logged("vllm_lmcache", 60.0);
    let log = r.log.as_ref().unwrap();
    assert_eq!(
        statuses(log, SESSION, 1),
        ["waiting", "running prefill", "running decode", "finished"]
    );
    let t = transfers(log, SESSION, 1);
    assert_eq!(
        t.iter().map(|x| (x.0, x.1)).collect::<Vec<_>>(),
        [
            (Transfer::Save, Moved::Registered),
            (Transfer::Save, Moved::Start),
            (Transfer::Save, Moved::Done)
        ]
    );
    assert!(status(&r, Status::Finished).stays > 0);
    assert!(status(&r, Status::WaitingForKv).stays > 0);
}

/// A program that calls no engine has no request: no status in the log,
/// and no table in the report (its stages and pools say where its sessions
/// are).
#[test]
fn a_program_without_an_engine_has_no_request() {
    let r = logged("pd_two_fabrics", 5.0);
    let log = r.log.as_ref().unwrap();
    assert!(!log.events.iter().any(|e| matches!(
        e.kind,
        EventKind::Status { .. } | EventKind::Transfer { .. }
    )));
    assert!(r.statuses.iter().all(|x| x.stays == 0 && x.number == 0.0));
    assert!(!r.text().contains("\nstatus "), "{}", r.text());
}

/// A resident stalled for room to grow (`preempt none`) is running all the
/// same: in vLLM's batch, not advancing. The engine's residents are the
/// running requests.
#[test]
fn the_running_requests_are_the_engine_s_residents() {
    for name in ["tgi", "vllm"] {
        let (p, base) = load(&serq::program_path(name), common::horizon(200.0));
        let r = run_ir(&p, Some(&base)).unwrap();
        let engine = r.stages.iter().find(|s| s.iterations > 0).unwrap();
        let running = status(&r, Status::Running).number;
        assert!(
            (running - engine.mean_number).abs() <= 1e-9 * running.max(1.0),
            "{name}: {running} {}",
            engine.mean_number
        );
    }
}

/// In every example a request has one status at a time: two status events
/// in a row about one request differ, a request's events end at
/// `finished`, and every transfer registered is done or still in flight at
/// the end, in order. The report is the same with or without the log.
#[test]
fn every_request_has_one_status_at_a_time() {
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
            type Key = (u64, Option<u32>, u32);
            let mut last: std::collections::HashMap<Key, (Status, RunMode)> = Default::default();
            let mut moved: std::collections::HashMap<(Key, &str), Moved> = Default::default();
            for e in &log.events {
                match &e.kind {
                    EventKind::Status {
                        who,
                        call,
                        status,
                        mode,
                        ..
                    } => {
                        let k = (who.serial, who.leg, *call);
                        let prev = last.insert(k, (*status, *mode));
                        assert_ne!(prev, Some((*status, *mode)), "{}", path.display());
                        assert!(
                            !matches!(prev, Some((Status::Finished, _))),
                            "{}: a request after its finish",
                            path.display()
                        );
                    }
                    EventKind::Transfer {
                        who,
                        call,
                        kind,
                        moved: m,
                        ..
                    } => {
                        let k = ((who.serial, who.leg, *call), kind.name());
                        let prev = moved.insert(k, *m);
                        let ok = match m {
                            Moved::Registered => prev.is_none(),
                            Moved::Start => prev == Some(Moved::Registered),
                            Moved::Done => prev.is_some_and(|p| p != Moved::Done),
                        };
                        assert!(ok, "{}: {k:?} {prev:?} then {m:?}", path.display());
                    }
                    _ => {}
                }
            }
            let plain = run_ir(&p, Some(&base)).unwrap();
            for (a, b) in plain.statuses.iter().zip(&r.statuses) {
                assert_eq!(a.stays, b.stays, "{}", path.display());
            }
            n += 1;
        }
    }
    assert!(n >= 30, "{n} programs");
}
