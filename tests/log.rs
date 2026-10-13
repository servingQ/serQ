//! The log of a run (`serq::engine::log`): what each session did, when,
//! kept only when asked for, and changing nothing else.

mod common;

use serq::engine::log::{Kind, Who};
use serq::frontend::ast::RunMode;
use serq::{run_ir, run_ir_logged};

/// `(what, where)` of each event about `who`, the names of the IR's
/// stages and pools in place of their indices.
fn story(log: &serq::engine::log::Log, who: Who) -> Vec<(String, String)> {
    let stage = |k: &usize| {
        let (name, index) = &log.stages[*k];
        index.map_or(name.clone(), |i| format!("{name}[{i}]"))
    };
    let pool = |k: &usize| {
        let (name, index) = &log.pools[*k];
        index.map_or(name.clone(), |i| format!("{name}[{i}]"))
    };
    log.events
        .iter()
        .filter_map(|e| match &e.kind {
            Kind::Arrive { who: w } if *w == who => Some(("arrive".into(), String::new())),
            Kind::Turn { who: w, turn } if *w == who => Some(("turn".into(), turn.to_string())),
            Kind::Wait { who: w, pool: q } if *w == who => Some(("wait".into(), pool(q))),
            Kind::Admit { who: w, pool: q } if *w == who => Some(("admit".into(), pool(q))),
            Kind::Run {
                who: w,
                stages,
                mode,
            } if *w == who => {
                let m = match mode {
                    RunMode::Plain => "run",
                    RunMode::Prefill => "prefill",
                    RunMode::Decode => "decode",
                };
                let at: Vec<String> = stages.iter().map(stage).collect();
                Some((m.into(), at.join("+")))
            }
            Kind::Done { who: w, stage: s } if *w == who => Some(("done".into(), stage(s))),
            Kind::End { who: w } if *w == who => Some(("end".into(), String::new())),
            _ => None,
        })
        .collect()
}

/// `llmd_nixl_pull.sq`'s first session, on its first turn: the router sends
/// a cold prompt (`prompt - hitD >= thr`) to a prefiller, whose request
/// waits in `P.reqs` (the first pool of its hold) and prefills; the
/// decoder's `decode … from P` holds `D.kv` before the read, which is the
/// link's latency and then the transfer over both NICs (`kvt.nixl_pull`:
/// `D.nic.latency`, then `P.nic` and `D.nic` as one flow); the decoder's
/// request then waits in `D.reqs`, computes its last token again (a
/// prefill of one token: `load_ended`) and decodes; the session goes to
/// its tool. Every step is one event, in order, and nothing is logged twice.
#[test]
fn a_session_is_logged_through_prefiller_link_and_decoder() {
    let p = serq::load(
        &serq::program_path("llmd_nixl_pull"),
        &common::horizon(30.0),
    )
    .unwrap();
    let r = run_ir_logged(&p, None, usize::MAX).unwrap();
    let log = r.log.as_ref().expect("a logged run keeps its log");
    assert!(!log.truncated);
    let first = Who {
        serial: 0,
        leg: None,
    };
    let s = story(log, first);
    let want: Vec<(String, String)> = [
        ("arrive", ""),
        ("turn", "1"),
        ("wait", "P.reqs[0]"),
        ("admit", "P.reqs[0]"),
        ("prefill", "P[0]"),
        ("done", "P[0]"),
        ("wait", "D.kv[0]"),
        ("admit", "D.kv[0]"),
        ("run", "D.nic.latency[0]"),
        ("done", "D.nic.latency[0]"),
        ("run", "P.nic[0]+D.nic[0]"),
        ("done", "P.nic[0]"),
        ("wait", "D.reqs[0]"),
        ("admit", "D.reqs[0]"),
        ("prefill", "D[0]"),
        ("done", "D[0]"),
        ("decode", "D[0]"),
        ("done", "D[0]"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(&s[..want.len()], &want[..], "{s:#?}");
    // then its tool, or its end: the workload's `more` draw
    assert!(
        matches!(s[want.len()].0.as_str(), "run" | "end"),
        "{:?}",
        s[want.len()]
    );
    // the times do not go back
    assert!(log.events.windows(2).all(|w| w[0].t <= w[1].t));
    // every iteration of an engine is logged with its batch: as many as
    // the report counts, and a prefill's tokens are its prompt's
    let iters = log
        .events
        .iter()
        .filter(|e| matches!(e.kind, Kind::Iter { .. }))
        .count();
    let counted: u64 = r.stages.iter().map(|s| s.iterations).sum();
    assert_eq!(iters as u64, counted);
    let first_iter = log
        .events
        .iter()
        .find_map(|e| match &e.kind {
            Kind::Iter {
                stage,
                tokens,
                reqs,
                dur,
            } => Some((*stage, *tokens, reqs.clone(), *dur)),
            _ => None,
        })
        .unwrap();
    assert_eq!(log.stages[first_iter.0].0, "P");
    assert_eq!(first_iter.2, vec![0]);
    assert!(
        first_iter.1 >= 1000.0 && first_iter.1 <= 3000.0,
        "{first_iter:?}"
    );
    assert!(first_iter.3 > 0.0);
}

/// Keeping the log changes nothing in the run: the report is the same,
/// field for field (the JSON without the `log` key).
#[test]
fn the_log_does_not_change_the_run() {
    let p = serq::load(
        &serq::program_path("llmd_nixl_pull"),
        &common::horizon(30.0),
    )
    .unwrap();
    let plain = run_ir(&p, None).unwrap().json();
    let logged = run_ir_logged(&p, None, usize::MAX).unwrap();
    let mut j: serde_json::Value = serde_json::from_str(&logged.json()).unwrap();
    let log = j
        .as_object_mut()
        .unwrap()
        .remove("log")
        .expect("the log is in the JSON");
    assert_eq!(log["version"], 0);
    assert_eq!(log["stages"][0]["name"], "P");
    assert_eq!(log["stages"][0]["index"], 0);
    assert_eq!(log["events"][0]["ev"], "arrive");
    assert_eq!(log["events"][0]["s"], 0);
    let plain: serde_json::Value = serde_json::from_str(&plain).unwrap();
    assert_eq!(j, plain);
}

/// Past the cap the log stops and says so; the run goes on to its end.
#[test]
fn the_cap_bounds_the_log_not_the_run() {
    let p = serq::load(
        &serq::program_path("llmd_nixl_pull"),
        &common::horizon(30.0),
    )
    .unwrap();
    let r = run_ir_logged(&p, None, 100).unwrap();
    let log = r.log.as_ref().unwrap();
    assert_eq!(log.events.len(), 100);
    assert!(log.truncated);
    assert_eq!(r.end, 30.0);
    assert_eq!(r.events, run_ir(&p, None).unwrap().events);
}

/// `serq run … --log FILE` writes the log as JSON to FILE, with the report
/// as usual on stdout.
#[test]
fn the_cli_writes_the_log_to_a_file() {
    let fx = common::Fixture::new();
    fx.write("p.sq", &common::main_source(common::PROGRAM));
    let out = fx.run(&[
        "run",
        "p.sq",
        "--horizon",
        "10",
        "--log",
        "log.json",
        "--json",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(report.get("log").is_some());
    let text = std::fs::read_to_string(fx.0.join("log.json")).unwrap();
    let log: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(log, report["log"]);
    let evs: Vec<&str> = log["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["ev"].as_str().unwrap())
        .collect();
    assert_eq!(evs, ["arrive", "turn", "run", "done", "end"]);
    assert_eq!(log["events"][2]["stages"], serde_json::json!([0]));
}
