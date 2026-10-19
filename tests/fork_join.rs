//! `fork { … }`: a leg of the turn runs beside the session (vLLM's push
//! proxy sends the prefill and the decode request at once). A leg calls an
//! engine whose requests are sent, and the session waits for it only by
//! reading that send: the decoder's load waits for KV until the leg's
//! prefill has finished (`docs/design/request-state.md`). `join;` is gone.

mod common;

/// One session of one request.
const ONE: &str = "workload { arrive batch(1); session { turn; end; } }";

fn run(src: &str, workload: &str) -> serq::Report {
    common::run(&common::main_source(src), workload, &common::horizon(100.0))
        .unwrap_or_else(|e| panic!("{e}\n{src}\n{workload}"))
}

fn fails(src: &str, workload: &str) -> String {
    match common::run(&common::main_source(src), workload, &common::horizon(100.0)) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("ran:\n{src}\n{workload}"),
    }
}

fn refused(src: &str, workload: &str, needle: &str) {
    let e = match common::compile(&common::main_source(src), workload, &common::horizon(100.0))
        .map(|_| ())
    {
        Err(e) => e.to_string(),
        Ok(_) => panic!("accepted:\n{src}"),
    };
    assert!(e.contains(needle), "{src}\n  {e}");
}

fn samples(r: &serq::Report, name: &str) -> Vec<f64> {
    r.observe(name).unwrap().samples.clone()
}

/// A push P/D pair: the prefiller's engine computes a token per second, the
/// decoder's reads the prompt over two NICs of 10 tokens/s after a wait of
/// `latency` 1. `route` is the gateway's route, `kv` the two KV capacities.
fn push(route: &str, kv: u32, per_token: f64) -> String {
    format!(
        r#"
        queue gw : gateway {{ route {{ {route} }} }}
        queue P : prefill {{
          device gpu {{ kv cap {kv}; }}
          engine on gpu {{
            reqs cap 4;
            tokens cap inf;
            scheduler {{ advance running; admit waiting while (running.preempted == 0) taking reqs (1), kv (request.known) cache (request.input); }}
            worker {{ forward (batch.tokens * {per_token}); }}
          }}
          pool reqs on P {{ }}
          pool kv on gpu {{ }}
          nic ps(10);
          prefill (p) {{ P (p, 1); }}
        }}
        queue D : agg, decode {{
          device gpu {{ kv cap {kv}; }}
          engine on gpu {{
            reqs cap 4;
            tokens cap inf;
            connector P {{ load (request.input - request.hit); }}
            scheduler {{ advance running; admit waiting while (running.preempted == 0) taking kv (request.known), reqs (request.loaded > 0 ? 0 : 1) cache (request.input); }}
            worker {{ forward (1); }}
          }}
          pool reqs on D {{ }}
          pool kv on gpu {{ }}
          nic ps(10);
          decode (p) {{ D (p, 2); }}
          decode (p) from P {{ D (p, 2); }}
        }}
        use "std/kvt"; use "std/share"; kvt.nixl_push (P, D, latency = 1, share = share.maxmin);
        server (prompt) {{ gw.route(); }}
    "#
    )
}

/// The decoder's request is admitted to its blocks at once and waits for
/// KV while the leg prefills (10 s); the push starts when the prefill's
/// send is there and takes the wait and a second. In order, the decoder is
/// admitted when the prefill has ended. The KV arrives at 12 either way.
#[test]
fn the_decoder_waits_for_the_leg_s_send() {
    let route = |concurrent: u32| {
        format!(
            "set t0 = now;
             if ({concurrent}) {{ fork {{ P.prefill (prompt); }} }} else {{ P.prefill (prompt); }}
             D.decode (prompt) from P;
             observe admitted_at = D.admitted - t0;
             observe loaded_at = D.load_done - t0;"
        )
    };
    let workload = "workload { arrive batch(1); init { set prompt = 10; } send (prompt); session { turn; end; } }";
    let r = run(&push(&route(1), 100, 1.0), workload);
    assert_eq!(samples(&r, "admitted_at"), [0.0]);
    assert_eq!(samples(&r, "loaded_at"), [12.0]);
    let r = run(&push(&route(0), 100, 1.0), workload);
    assert_eq!(samples(&r, "admitted_at"), [10.0]);
    assert_eq!(samples(&r, "loaded_at"), [12.0]);
}

/// The client of the crossing requests below: the second session arrives
/// first.
const CROSSING: &str = "stage client_gate : delay;
        workload {
          arrive batch(2);
          init { set prompt = 8; }
          send (prompt);
          session { run client_gate (cost(client_gate, serial == 0 ? 0.5 : 0)); turn; end; }
        }";

/// Two turns whose requests wait for each other: A's decoder request holds
/// the decoder's memory waiting for A's send, A's prefill waits in the
/// prefiller's queue for its memory, which B's send holds until B's decoder
/// reads it, which waits for the decoder's memory. The run says so instead of ending
/// as if the requests were merely slow.
#[test]
fn requests_that_wait_for_each_other_fail_the_run() {
    let route = "fork { P.prefill (prompt); }
            run gate (cost(gate, serial == 1 ? 1 : 0));
            D.decode (prompt) from P;";
    let src = format!("{}\n        stage gate : delay;", push(route, 10, 0.01));
    let e = fails(&src, CROSSING);
    assert!(e.contains("wait for each other"), "{e}");
    assert!(
        e.contains("session 0 waits for the send its load reads"),
        "{e}"
    );
    assert!(e.contains("a leg of session 0 waits at `P.reqs`"), "{e}");
    assert!(e.contains("session 1 waits at `D.kv`"), "{e}");
}

/// A session that ends while a leg it forked runs is an error, which no
/// static check sees here: the read that would have waited for the leg is
/// on a way the run does not take.
#[test]
fn a_session_may_not_end_before_its_legs() {
    let route = "set x = 0; fork { P.prefill (prompt); } if (x) { D.decode (prompt) from P; }";
    let workload = "workload { arrive batch(1); init { set prompt = 10; } send (prompt); session { turn; end; } }";
    let e = fails(&push(route, 100, 1.0), workload);
    assert!(e.contains("while a leg it forked runs"), "{e}");
}

/// `join;` is gone, and says what replaced it.
#[test]
fn join_is_refused_with_what_replaced_it() {
    refused(
        "stage svc : delay;
        server { run svc (cost(svc, 1)); join;
        }
        ",
        ONE,
        "`join;` is gone",
    );
}

/// A leg calls an engine whose requests are sent: the session waits for a
/// leg only by reading its send, and a leg that leaves none would run on
/// unwaited for.
#[test]
fn a_leg_calls_a_source_engine() {
    refused(
        "stage svc : delay;
        server { fork { run svc (cost(svc, 5)); } run svc (cost(svc, 1));
        }
        ",
        ONE,
        "calls no engine whose requests are sent",
    );
}

/// A leg is a part of the turn: it may not turn, end or fork.
#[test]
fn a_leg_may_only_run_the_request() {
    for (stmt, what) in [
        ("turn;", "`turn`"),
        ("end;", "`end`"),
        ("fork { run svc (cost(svc, 1)); }", "fork"),
    ] {
        refused(
            "server {}",
            // the client's own `svc`: a leg of the session
            &format!(
                "stage svc : delay;
        workload {{ arrive batch(1); session {{
            fork {{ run svc (cost(svc, 1)); {stmt} }} turn; end;
        }} }}"
            ),
            &format!("a `fork`'s leg may not {what}"),
        );
    }
}
