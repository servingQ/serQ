//! What the client sees (`docs/design/client-metrics.md`): every turn's
//! `ttft`, `tpot` and `response`, in the report's `client` section, with no
//! program text.

mod common;

use std::collections::HashMap;

use serq::frontend::parser::parse_expr;
use serq::{Overrides, compile_source_at, program_path, run_ir, run_source};

fn run(src: &str) -> serq::Report {
    run_source(&common::main_source(src), &common::horizon(100.0), None)
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn samples(r: &serq::Report, name: &str) -> Vec<f64> {
    r.client(name).unwrap().samples.clone()
}

const ENGINE: &str = "
device gpu { }
engine llm on gpu {
  tokens cap 8;
  schedule { advance running; admit waiting while (running.preempted == 0); }
  execute (1);
}
";

/// One request: a prefill of 8 tokens in the first iteration samples the
/// first token at 1; the decode's two tokens come at 2 and 3.
#[test]
fn a_request_s_first_token_and_the_tokens_after_it() {
    let r = run(&format!(
        "{ENGINE}
workload {{ arrive batch(1); }}
server {{ run llm prefill (cost(llm, 8)); run llm decode (cost(llm, 2)); }}"
    ));
    assert_eq!(samples(&r, "ttft"), [1.0]);
    assert_eq!(samples(&r, "tpot"), [1.0]);
    assert_eq!(samples(&r, "response"), [3.0]);
}

/// A turn is sent at its `turn;` and answered when the server's statements
/// are done: the client's think time after it is not the turn's.
#[test]
fn a_turn_ends_where_the_server_does() {
    let r = run(&format!(
        "{ENGINE}
stage pause : delay;
workload {{ arrive batch(1); session {{ turn; run pause (cost(pause, 5)); turn; }} }}
server {{ run llm prefill (cost(llm, 8)); run llm decode (cost(llm, 1)); }}"
    ));
    assert_eq!(samples(&r, "ttft"), [1.0, 1.0]);
    assert_eq!(samples(&r, "response"), [2.0, 2.0]);
}

/// A station with no engine answers a turn and sends no token.
#[test]
fn a_turn_that_decodes_nothing_has_a_response_only() {
    let r = run("stage svc : fifo;
workload { arrive batch(2); }
server { run svc (cost(svc, 2)); }");
    assert_eq!(samples(&r, "response"), [2.0, 4.0]);
    assert_eq!(r.client("ttft").unwrap().count, 0);
    assert_eq!(r.client("tpot").unwrap().count, 0);
}

/// A decode with no prefill before it: its first token is the client's
/// first, at the end of the first iteration, and two tokens follow.
#[test]
fn a_decode_alone_samples_the_first_token() {
    let r = run(&format!(
        "{ENGINE}
workload {{ arrive batch(1); }}
server {{ run llm decode (cost(llm, 3)); }}"
    ));
    assert_eq!(samples(&r, "ttft"), [1.0]);
    assert_eq!(samples(&r, "tpot"), [1.0]);
    assert_eq!(samples(&r, "response"), [3.0]);
}

/// A leg forked in one turn and joined in the next works for the first:
/// its tokens are no token of the second, which only waits for it.
#[test]
fn a_leg_works_for_the_turn_it_was_forked_in() {
    let r = run(&format!(
        "{ENGINE}
workload {{ arrive batch(1); session {{ turn; turn; }} }}
server {{
  branch (turn_no == 1) {{ fork {{ run llm decode (cost(llm, 4)); }} }} else {{ join; }}
}}"
    ));
    assert_eq!(samples(&r, "response"), [0.0, 4.0]);
    assert_eq!(r.client("ttft").unwrap().count, 0);
}

/// One output token: a first token, and no time per token after it.
#[test]
fn one_output_token_has_no_tpot() {
    let r = run(&format!(
        "{ENGINE}
workload {{ arrive batch(1); }}
server {{ run llm prefill (cost(llm, 8)); run llm decode (cost(llm, 0)); }}"
    ));
    assert_eq!(samples(&r, "ttft"), [1.0]);
    assert_eq!(r.client("tpot").unwrap().count, 0);
}

/// Prefill on one engine, decode on another: the prefiller's token is not
/// the client's, the decoder's first is (llm-d's sidecar, `max_tokens = 1`).
#[test]
fn the_first_token_is_the_decoder_s() {
    let r = run("
device g1 { }
device g2 { }
engine p on g1 { tokens cap 8; schedule { advance running; admit waiting while (running.preempted == 0); } execute (1); }
engine d on g2 { tokens cap 8; schedule { advance running; admit waiting while (running.preempted == 0); } execute (1); }
workload { arrive batch(1); }
server {
  run p prefill (cost(p, 8));
  run d prefill (cost(d, 1));
  run d decode (cost(d, 2));
}");
    assert_eq!(samples(&r, "ttft"), [2.0]);
    assert_eq!(samples(&r, "tpot"), [1.0]);
    assert_eq!(samples(&r, "response"), [4.0]);
}

/// A program's own `observe ttft` keeps its name, in its own table.
#[test]
fn a_program_s_observe_keeps_its_name() {
    let r = run(&format!(
        "{ENGINE}
workload {{ arrive batch(1); }}
server {{ run llm prefill (cost(llm, 8)); observe ttft = 42; run llm decode (cost(llm, 2)); }}"
    ));
    assert_eq!(r.observe("ttft").unwrap().samples, [42.0]);
    assert_eq!(samples(&r, "ttft"), [1.0]);
    assert!(r.text().contains("\nclient "), "{}", r.text());
}

/// `pd_batching.sq` writes the three by hand, through preemptions and
/// remote prefills, and the client's are its own: `tpot` and `response`
/// sample by sample, `ttft` turn by turn (the program records it when the
/// turn ends, the client at the first token).
#[test]
fn the_client_s_are_pd_batching_s_hand_written_ones() {
    let path = program_path("pd_batching");
    let src = std::fs::read_to_string(&path).unwrap();
    for (seed, mode) in [(1, 0.0), (2, 1.0)] {
        let ov = Overrides {
            seed: Some(seed),
            horizon: Some(30.0),
            warmup: Some(0.0),
            lets: vec![("mode".into(), parse_expr(&mode.to_string()).unwrap())],
            ..Default::default()
        };
        let p = compile_source_at(&src, path.parent(), &ov).unwrap();
        let r = run_ir(&p, path.parent()).unwrap();
        for name in ["tpot", "response"] {
            let (mine, client) = (r.observe(name).unwrap(), r.client(name).unwrap());
            assert_eq!(mine.samples, client.samples, "{name}, mode {mode}");
            assert_eq!(mine.records, client.records, "{name}, mode {mode}");
        }
        let by_turn = |o: &serq::engine::report::ObserveReport| -> HashMap<(u64, u32), f64> {
            o.records
                .iter()
                .map(|r| (r.1, r.2))
                .zip(o.samples.iter().copied())
                .collect()
        };
        let (mine, client) = (
            by_turn(r.observe("ttft").unwrap()),
            by_turn(r.client("ttft").unwrap()),
        );
        assert!(mine.len() > 100, "{} turns", mine.len());
        for (turn, v) in &mine {
            assert_eq!(client.get(turn), Some(v), "ttft of {turn:?}, mode {mode}");
        }
        // the rest had their first token and no answer when the run ended
        let answered = r.client("response").unwrap().records.len();
        assert_eq!(answered, mine.len(), "mode {mode}");
    }
}
