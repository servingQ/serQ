//! The log of a run (`serq::engine::log`): what each session did, when,
//! kept only when asked for, and changing nothing else.

mod common;

use serq::engine::log::{EventKind, Log, Who};
use serq::frontend::ast::RunMode;
use serq::ir::CStageKind;
use serq::{run_ir, run_ir_logged};
use std::path::{Path, PathBuf};

/// The example at `path` with the workload `examples/pairs.json` gives it,
/// and the directory its trace is read from: the workload's.
fn load(path: &Path, ov: serq::Overrides) -> (serq::Program, PathBuf) {
    let p = match serq::paired_workload(path) {
        Some(w) => serq::load_paired(path, &w, &ov).map(|p| (p, w.parent().unwrap().to_path_buf())),
        None => serq::load(path, &ov).map(|p| (p, path.parent().unwrap().to_path_buf())),
    };
    p.unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `(what, where)` of each event about `who`, the names of the IR's
/// stages and pools in place of their indices.
fn story(log: &serq::engine::log::Log, who: Who) -> Vec<(String, String)> {
    let stage = |k: &usize| log.stages[*k].label();
    let pool = |k: &usize| log.pools[*k].label();
    log.events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Arrive { who: w } if *w == who => Some(("arrive".into(), String::new())),
            EventKind::Turn { who: w, turn } if *w == who => {
                Some(("turn".into(), turn.to_string()))
            }
            EventKind::Wait { who: w, pool: q } if *w == who => Some(("wait".into(), pool(q))),
            EventKind::Admit { who: w, pool: q } if *w == who => Some(("admit".into(), pool(q))),
            EventKind::Run {
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
            EventKind::Done { who: w, stage: s } if *w == who => Some(("done".into(), stage(s))),
            EventKind::End { who: w } if *w == who => Some(("end".into(), String::new())),
            _ => None,
        })
        .collect()
}

/// `llmd_nixl_pull.sq`'s first session, on its first turn: the router sends
/// a cold prompt (`prompt - hitD >= thr`) to a prefiller, whose request
/// waits in `P.reqs` (the first pool of its hold) and prefills; the
/// decoder's `decode … from P` holds `D.kv` before the read, the transfer
/// over both NICs (`kvt.nixl_pull`: `P.nic` and `D.nic` as one flow, its
/// latency waited at them first, inside the one run); the decoder's
/// request then waits in `D.reqs`, computes its last token again (a
/// prefill of one token: `load_ended`) and decodes; the session goes to
/// its tool. Every step is one event, in order, and nothing is logged twice.
#[test]
fn a_session_is_logged_through_prefiller_link_and_decoder() {
    let (p, _) = load(&serq::program_path("llmd_nixl_pull"), common::horizon(30.0));
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
        .filter(|e| matches!(e.kind, EventKind::Iter { .. }))
        .count();
    let counted: u64 = r.stages.iter().map(|s| s.iterations).sum();
    assert_eq!(iters as u64, counted);
    let first_iter = log
        .events
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::Iter {
                stage,
                tokens,
                reqs,
                dur,
                ..
            } => Some((*stage, *tokens, reqs.clone(), *dur)),
            _ => None,
        })
        .unwrap();
    assert_eq!(log.stages[first_iter.0].name, "P");
    assert_eq!(first_iter.2, vec![first]);
    // the first turn's prompt: `K + n` with `K = 0` and `n = ceil(~uniform(1000,
    // 3000))`, all of it prefilled since the cache is cold (`c = cached` = 0)
    assert!(
        first_iter.1 >= 1000.0 && first_iter.1 <= 3000.0,
        "{first_iter:?}"
    );
    assert!(first_iter.3 > 0.0);
}

/// Every example program (`examples/*/*.sq`); a workload's file runs with
/// the program it is paired with, so `examples/workloads` is not one.
fn examples() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut paths = vec![];
    for group in std::fs::read_dir(&root).unwrap().flatten() {
        if !group.path().is_dir() || group.file_name() == "workloads" {
            continue;
        }
        for entry in std::fs::read_dir(group.path()).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "sq") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    assert!(paths.len() >= 30, "{} programs", paths.len());
    paths
}

/// Keeping the log changes nothing in the run: the report is the same,
/// field for field (the JSON without the `log` key), for every example
/// program, with its preemptions, forks and lease revocations.
#[test]
fn the_log_does_not_change_the_run() {
    for path in examples() {
        let (p, base) = load(&path, common::horizon(20.0));
        let base = Some(base.as_path());
        let plain = run_ir(&p, base).unwrap().json();
        let logged = run_ir_logged(&p, base, usize::MAX).unwrap();
        let mut j: serde_json::Value = serde_json::from_str(&logged.json()).unwrap();
        let log = j
            .as_object_mut()
            .unwrap()
            .remove("log")
            .expect("the log is in the JSON");
        assert_eq!(log["version"], 0, "{}", path.display());
        let plain: serde_json::Value = serde_json::from_str(&plain).unwrap();
        assert_eq!(j, plain, "{}", path.display());
    }
}

/// The log reads back as it was written (`Log::from_json`), for every
/// example program: what `serq draw --log` draws is what the run logged.
/// And it shows the pools' invariant: an engine's memory never holds more
/// than its pool's cap, allocated and cached together, which the header
/// gives; what it has evicted only grows.
#[test]
fn the_log_reads_back_and_keeps_the_memory_within_its_cap() {
    for path in examples() {
        let at = path.display();
        let (p, base) = load(&path, common::horizon(20.0));
        let r = run_ir_logged(&p, Some(base.as_path()), usize::MAX).unwrap();
        let log = r.log.unwrap();
        let back = Log::from_json(&log.json()).unwrap_or_else(|e| panic!("{at}: {e}"));
        assert_eq!(back.stages, log.stages, "{at}");
        assert_eq!(back.pools, log.pools, "{at}");
        assert_eq!(back.truncated, log.truncated, "{at}");
        assert!(
            back.events == log.events,
            "{at}: the events read back otherwise"
        );
        let mut evicted = vec![0.0; log.stages.len()];
        for e in &log.events {
            let EventKind::Iter {
                stage,
                memory: Some(m),
                ..
            } = &e.kind
            else {
                continue;
            };
            let pool = log.stages[*stage]
                .memory
                .unwrap_or_else(|| panic!("{at}: an engine's memory not in the header"));
            let cap = log.pools[pool].cap;
            assert!(
                m.used + m.cached <= cap,
                "{at} at {}: used {} + cached {} > cap {cap}",
                e.t,
                m.used,
                m.cached
            );
            assert!(m.evicted >= evicted[*stage], "{at} at {}", e.t);
            evicted[*stage] = m.evicted;
        }
    }
}

/// An iteration's `evicted` is the memory pool's `evicted_units` so far:
/// the last iteration's is at most the report's, which counts the whole
/// run (a warm-up does not reset it), and on the cliff's program at 2.0
/// sessions/s, where the cache is gone, it rises.
#[test]
fn an_iteration_counts_what_its_memory_evicted() {
    let mut ov = common::horizon(60.0);
    ov.set_num("Lambda", 2.0).unwrap();
    let (p, base) = load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/tutorial/programs/05-engine.sq"),
        ov,
    );
    let r = run_ir_logged(&p, Some(base.as_path()), usize::MAX).unwrap();
    let log = r.log.as_ref().unwrap();
    let kv = log.pools.iter().position(|q| q.name == "kv").unwrap();
    assert_eq!(log.stages[0].memory, Some(kv));
    assert_eq!(log.pools[kv].cap, 4000.0 * 16.0);
    let last = log
        .events
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EventKind::Iter {
                memory: Some(m), ..
            } => Some(m.evicted),
            _ => None,
        })
        .unwrap();
    let report = r.pools[kv].evicted_units;
    assert!(last > 0.0 && last <= report, "{last} of {report}");
}

/// An iteration names each request's tokens, its budget and its engine's
/// memory (#728), for every example program: `each` lines up with `reqs`
/// and sums to `tokens`, which the budget bounds; `used` and `cached` are
/// there exactly when the engine has a memory pool, and are units, so
/// never negative.
#[test]
fn an_iteration_gives_each_request_its_tokens_its_budget_and_its_memory() {
    let mut iters = 0;
    for path in examples() {
        let at = path.display();
        let (p, base) = load(&path, common::horizon(20.0));
        let r = run_ir_logged(&p, Some(base.as_path()), usize::MAX).unwrap();
        for e in &r.log.as_ref().unwrap().events {
            let EventKind::Iter {
                stage,
                tokens,
                budget,
                reqs,
                each,
                memory,
                ..
            } = &e.kind
            else {
                continue;
            };
            let CStageKind::Step(spec) = &p.stages[*stage].kind else {
                panic!(
                    "{at}: an iteration of `{}`, no engine",
                    p.stages[*stage].name
                )
            };
            assert_eq!(each.len(), reqs.len(), "{at} at {}", e.t);
            assert_eq!(each.iter().sum::<f64>(), *tokens, "{at} at {}", e.t);
            assert!(each.iter().all(|&n| n > 0.0), "{at} at {}: {each:?}", e.t);
            assert!(*tokens <= *budget, "{at} at {}: {tokens} > {budget}", e.t);
            assert_eq!(memory.is_some(), spec.memory.is_some(), "{at}");
            if let Some(m) = memory {
                assert!(
                    m.used >= 0.0 && m.cached >= 0.0 && m.evicted >= 0.0,
                    "{at} at {}: {memory:?}",
                    e.t
                );
            }
            iters += 1;
        }
    }
    assert!(iters > 0);
}

/// The budget is the `tokens cap` as each iteration reads it, so it is in
/// the event and not the header: `replica.sq`'s cap is `max(running.decoding,
/// floor((omega + beta * running.kv_decode) / a))`, `floor(omega / a)` = 10
/// until the decoders' context passes 10⁴ tokens (first at t = 20.66 at the
/// default seed) and more after (up to 12 by t = 40).
#[test]
fn the_budget_is_read_per_iteration() {
    let (p, base) = load(&serq::program_path("replica"), common::horizon(40.0));
    let r = run_ir_logged(&p, Some(base.as_path()), usize::MAX).unwrap();
    let budgets: Vec<f64> = r
        .log
        .as_ref()
        .unwrap()
        .events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Iter { budget, .. } => Some(*budget),
            _ => None,
        })
        .collect();
    let lo = budgets.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = budgets.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    assert_eq!(lo, 10.0, "floor(omega / a)");
    assert!(lo < hi, "{lo} .. {hi}");
}

/// A run's mode is in its `run` event, whether or not an engine request
/// holds it: a queue's entry that runs `llm decode` with no `request` has
/// no `status`, and `tools/serq_iter_trace.py` still prints its decodes as
/// `d`, as the removed `SERQ_TRACE_ITER` switch did (it read the job's mode)
/// and as main's binary prints these four lines. The tool once took the
/// mode from `status` alone and printed them `p1` (#737).
#[test]
fn a_decode_outside_an_engine_request_is_traced_as_a_decode() {
    let src = "let bs = 16;
        queue llm : agg {
          pool kv { cap 64 * bs; block bs; }
          device gpu { }
          engine on gpu {
            tokens cap 1000;
            scheduler { advance running; admit waiting while (running.preempted == 0); }
            worker { forward (1); }
          }
          decode (prompt) {
            hold kv (cost(kv, prompt)) {
              run llm prefill (cost(llm, prompt)) growing kv;
              run llm decode (cost(llm, o - 1)) growing kv;
            }
          }
        }
        server (prompt, hidden o) { llm.decode (prompt); }";
    let workload = "workload { arrive batch(2); init { set prompt = 32; set o = 4; }
          send (prompt, o);
          session { turn; end; } }";
    let p = common::compile(&common::main_source(src), workload, &common::horizon(50.0)).unwrap();
    let r = run_ir_logged(&p, None, usize::MAX).unwrap();
    let log = r.log.unwrap();
    assert!(
        !log.events
            .iter()
            .any(|e| matches!(e.kind, EventKind::Status { .. })),
        "no engine request, no status"
    );
    assert_eq!(
        common::iter_lines(&log),
        [
            "ITER 0.0000 0:1:p32 1:1:p32",
            "ITER 1.0000 0:1:d1 1:1:d1",
            "ITER 2.0000 0:1:d1 1:1:d1",
            "ITER 3.0000 0:1:d1 1:1:d1",
        ]
    );
}

/// A leg (`fork`) shares its session's serial: the log tells them apart
/// by the leg's number, in the events about it and in an engine's batch.
/// `vllm_nixl_push.sq` forks the prefill: its first session's leg 0 is
/// what `P` prefills while the session itself waits at the decoder.
#[test]
fn a_leg_is_told_from_its_session() {
    let path = serq::program_path("vllm_nixl_push");
    let (p, base) = load(&path, common::horizon(5.0));
    // the workload replays a trace file next to it
    let r = run_ir_logged(&p, Some(&base), usize::MAX).unwrap();
    let log = r.log.as_ref().unwrap();
    let leg = Who {
        serial: 0,
        leg: Some(0),
    };
    assert!(
        log.events
            .iter()
            .any(|e| matches!(&e.kind, EventKind::Fork { who } if *who == leg))
    );
    let first_p = log
        .events
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::Iter { stage, reqs, .. } if log.stages[*stage].name == "P" => {
                Some(reqs.clone())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(first_p, vec![leg]);
    let j: serde_json::Value = serde_json::from_str(&log.json()).unwrap();
    let events = j["events"].as_array().unwrap();
    let fork = events.iter().find(|e| e["ev"] == "fork").unwrap();
    assert_eq!(fork["s"], 0);
    assert_eq!(fork["leg"], 0);
    let iter = events
        .iter()
        .find(|e| e["ev"] == "iter" && e["reqs"].as_array().is_some_and(|r| !r.is_empty()))
        .unwrap();
    assert_eq!(iter["reqs"][0], serde_json::json!([0, 0]));
}

/// Past the cap the log stops and says so; the run goes on to its end.
#[test]
fn the_cap_bounds_the_log_not_the_run() {
    let (p, _) = load(&serq::program_path("llmd_nixl_pull"), common::horizon(30.0));
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
    fx.write("w.sq", common::WORKLOAD);
    let out = fx.run(&[
        "run",
        "p.sq",
        "--workload",
        "w.sq",
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
