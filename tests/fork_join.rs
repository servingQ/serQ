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
            scheduler {{ advance running; admit waiting while (running.preempted == 0) taking reqs (1), kv (request.known) cache kv (request.input); }}
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
            scheduler {{ advance running; admit waiting while (running.preempted == 0) taking kv (request.known), reqs (request.loaded > 0 ? 0 : 1) cache kv (request.input); }}
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

/// A leg's send is read on every way after its `fork` before the session
/// ends: here the read is on one way of an `if` only, and the session
/// could end while its leg runs. The program does not link.
#[test]
fn a_leg_s_send_is_read_on_every_way() {
    let route = "set x = 0; fork { P.prefill (prompt); } if (x) { D.decode (prompt) from P; }";
    let workload = "workload { arrive batch(1); init { set prompt = 10; } send (prompt); session { turn; end; } }";
    refused(
        &push(route, 100, 1.0),
        workload,
        "a leg's send of `P.kv` is read on no way after its `fork`",
    );
    // a leg calls its source once
    refused(
        &push(
            "fork { P.prefill (prompt); P.prefill (prompt); } D.decode (prompt) from P;",
            100,
            1.0,
        ),
        workload,
        "more than once",
    );
    // each leg's send is read: two legs of one source and one read leave
    // one unread (PR #719's fourth review: one read cleared both)
    refused(
        &push(
            "fork { P.prefill (prompt); } fork { P.prefill (prompt); } D.decode (prompt) from P;",
            100,
            1.0,
        ),
        workload,
        "is read on no way after its `fork`",
    );
    // a call that takes its KV from no source reads no send (its review)
    refused(
        &push("fork { P.prefill (prompt); } D.decode (prompt);", 100, 1.0),
        workload,
        "is read on no way after its `fork`",
    );
    // and leases nothing: what it leaves its session is the send
    refused(
        &push("D.decode (prompt);", 100, 1.0).replace(
            "server (prompt) { gw.route(); }",
            "pool extra { cap 10; }
        server (prompt) {
          fork { P.prefill (prompt); hold extra (cost(extra, 1)) { } lease extra (inf); }
          D.decode (prompt) from P;
        }",
        ),
        workload,
        "a `fork`'s leg may not lease",
    );
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

const PROMPT10: &str =
    "workload { arrive batch(1); init { set prompt = 10; } send (prompt); session { turn; end; } }";

/// `push` with a leg that ends a second after its prefill, its relation
/// closed by `ttl` (`, ttl = 0.5`, or nothing).
fn leg_outlives_prefill(ttl: &str) -> String {
    let route = "set t0 = now;
         fork { P.prefill (prompt); run gate (cost(gate, 1)); }
         D.decode (prompt) from P;
         observe done_at = now - t0;
         observe loaded_at = D.load_done - t0;";
    push(route, 100, 1.0).replace(
        "share = share.maxmin);",
        &format!("share = share.maxmin{ttl});\n        stage gate : delay;"),
    )
}

/// A leg whose send expired (`ttl = 0.5`) before it ended (at 11, a second
/// after its prefill): its session's load, waiting for a send no leg is
/// left to register, does not wait for ever (PR #719's first review) and
/// does not read the KV the prefiller has freed (its second): it takes its
/// running slot and computes the prompt itself, its prefill at 11-12 and
/// its decode at 12-13, where the read would have answered at 15.
#[test]
fn a_load_whose_send_expired_computes_the_prompt_itself() {
    let src = leg_outlives_prefill;
    let r = run(&src(", ttl = 0.5"), PROMPT10);
    assert_eq!(samples(&r, "done_at"), [13.0]);
    assert_eq!(r.stage("D.nic").map(|s| s.completed), Some(0));
    // what the decoder's admission recorded (10 to load, 1 to prefill: its
    // last token again) becomes what it did, not left as it was (PR #719's
    // third review): nothing loaded, the 10 tokens prefilled
    assert_eq!(samples(&r, "D.loaded"), [0.0]);
    assert_eq!(samples(&r, "D.prefill"), [10.0]);
    // and its load is over when it found no send, at 11, as the log says
    // (PR #719's fourth review: `load_done` stayed unset, the load
    // registered and never done)
    assert_eq!(samples(&r, "loaded_at"), [11.0]);
    let p = common::compile(
        &common::main_source(&src(", ttl = 0.5")),
        PROMPT10,
        &common::horizon(100.0),
    )
    .unwrap();
    let log = serq::run_ir_logged(&p, None, usize::MAX)
        .unwrap()
        .log
        .unwrap();
    use serq::engine::log::EventKind;
    use serq::engine::state::{Moved, Transfer};
    let loads: Vec<(Moved, f64)> = log
        .events
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::Transfer {
                kind: Transfer::Load,
                moved,
                ..
            } => Some((moved, e.t)),
            _ => None,
        })
        .collect();
    assert_eq!(loads, [(Moved::Registered, 0.0), (Moved::Done, 11.0)]);
    // without the `ttl`, the send is read: the copy, then the last token again
    let r = run(&src(""), PROMPT10);
    assert_eq!(samples(&r, "done_at"), [15.0]);
    assert_eq!(samples(&r, "D.loaded"), [10.0]);
    assert_eq!(samples(&r, "D.prefill"), [1.0]);
}

/// A `ttl` is folded by the parser, which knows only the `let`s above it,
/// and reaches the linker as a number; an aggregate's index in it is a name
/// the program does not already have, as in any expression. Until #561,
/// `sum prompt in 2 (0.25)` linked with `prompt` an attribute. The
/// relation's `latency` is a constant of the program, which the linker
/// asks the same; in either, so is a branch a constant condition does not
/// take (#561 review: the `latency` linked and the `ttl` did not).
#[test]
fn a_relations_aggregate_index_is_its_own() {
    let refusal = "`sum prompt in`: `prompt` is also a session attribute";
    for ttl in ["sum prompt in 2 (0.25)", "1 ? 0.5 : sum prompt in 2 (0.25)"] {
        refused(
            &leg_outlives_prefill(&format!(", ttl = {ttl}")),
            PROMPT10,
            refusal,
        );
    }
    for latency in ["sum prompt in 2 (0.5)", "1 ? 1 : sum prompt in 2 (0.5)"] {
        refused(
            &leg_outlives_prefill("").replace("latency = 1,", &format!("latency = {latency},")),
            PROMPT10,
            refusal,
        );
    }
    // an index of its own: the same `ttl` as `0.5`
    let r = run(&leg_outlives_prefill(", ttl = sum k in 2 (0.25)"), PROMPT10);
    assert_eq!(samples(&r, "done_at"), [13.0]);
}

/// The load ends the send its copy started on, not a later one: leg A's
/// prefill ends at 10 and the copy (10 tokens over NICs of 1/s, after its
/// wait of 1) runs to 21, while leg B's prefill, queued behind A's, sends at
/// 20. The send that ends with the load is A's; B's is left to the
/// session's end. (PR #719's review: the load used to look up the latest
/// send again when it ended, which would have been B's.)
#[test]
fn a_load_ends_the_send_its_copy_read() {
    use serq::engine::log::EventKind;
    use serq::engine::state::{Moved, Transfer};
    let route = "fork { P.prefill (prompt); }
         fork { run gate (cost(gate, 0.5)); P.prefill (prompt); }
         D.decode (prompt) from P;
         D.decode (prompt) from P;";
    let src = push(route, 100, 1.0).replace("nic ps(10);", "nic ps(1);")
        + "\n        stage gate : delay;";
    let p = common::compile(
        &common::main_source(&src),
        PROMPT10,
        &common::horizon(100.0),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let r = serq::run_ir_logged(&p, None, usize::MAX).unwrap();
    let log = r.log.as_ref().unwrap();
    let load_done = log
        .events
        .iter()
        .find(|e| {
            matches!(
                e.kind,
                EventKind::Transfer {
                    kind: Transfer::Load,
                    moved: Moved::Done,
                    ..
                }
            )
        })
        .unwrap()
        .t;
    assert_eq!(load_done, 21.0);
    let sends: Vec<(Option<u32>, Moved, f64)> = log
        .events
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::Transfer {
                who,
                kind: Transfer::Send,
                moved,
                ..
            } => Some((who.leg, moved, e.t)),
            _ => None,
        })
        .collect();
    assert!(sends.contains(&(Some(0), Moved::Start, 10.0)), "{sends:?}");
    assert!(
        sends.contains(&(Some(1), Moved::Registered, 20.0)),
        "{sends:?}"
    );
    assert!(sends.contains(&(Some(0), Moved::Done, 21.0)), "{sends:?}");
    assert!(!sends.contains(&(Some(1), Moved::Done, 21.0)), "{sends:?}");
}

/// Two relations from one source name one `ttl`: a request of the source
/// keeps its blocks for one send, whichever reader reads it.
#[test]
fn two_relations_from_one_source_name_one_ttl() {
    let reader = |name: &str| {
        format!(
            "queue {name} : agg, decode {{
          device gpu {{ kv cap 100; }}
          engine on gpu {{
            reqs cap 4;
            tokens cap inf;
            connector P {{ load (request.input - request.hit); }}
            scheduler {{ advance running; admit waiting while (running.preempted == 0) taking kv (request.known), reqs (request.loaded > 0 ? 0 : 1) cache kv (request.input); }}
            worker {{ forward (1); }}
          }}
          pool reqs on {name} {{ }}
          pool kv on gpu {{ }}
          nic ps(10);
          decode (p) {{ {name} (p, 2); }}
          decode (p) from P {{ {name} (p, 2); }}
        }}"
        )
    };
    let src = |t1: &str, t2: &str| {
        push(
            "P.prefill (prompt); if (prompt > 5) { D.decode (prompt) from P; } else { E.decode (prompt) from P; }",
            100,
            1.0,
        )
        .replace(
            "use \"std/kvt\";",
            &format!("{}\n        use \"std/kvt\";", reader("E")),
        )
        .replace(
            "kvt.nixl_push (P, D, latency = 1, share = share.maxmin);",
            &format!(
                "kvt.nixl_pull (P, D, share = share.maxmin, ttl = {t1}); \
                 kvt.nixl_pull (P, E, share = share.maxmin, ttl = {t2});"
            ),
        )
    };
    run(&src("2", "2"), PROMPT10);
    let e = fails(&src("1", "2"), PROMPT10);
    // where the second relation is written (PR #719's fourth review)
    assert!(
        e.starts_with("deployment.sq: ") && e.contains("kvt.nixl_pull (P, E"),
        "{e}"
    );
    assert!(
        e.contains("two KV relations from `P` name different `ttl`s"),
        "{e}"
    );
}

/// A leg shares its session's cached prefix: the second turn's prefill leg
/// reads from `P.kv` what the first turn's left cached there (the session's
/// own entry, its chain the session's), and computes only the rest.
#[test]
fn a_leg_reads_the_session_s_cached_prefix() {
    let route = "fork { P.prefill (prompt); } D.decode (prompt) from P;";
    let workload = "workload { arrive batch(1); init { set k = 0; } turn { set prompt = 16 * (k + 1); set k = k + 1; } send (prompt); session { turn; turn; end; } }";
    let r = run(
        &push(route, 1000, 1.0).replace("pool kv on gpu { }", "pool kv on gpu { block 16; }"),
        workload,
    );
    assert_eq!(samples(&r, "P.reused"), [0.0, 16.0], "{}", r.text());
}

/// A leg that calls its source once on each way of an `if` calls it once:
/// its send is read once (CodeRabbit on #719: both ways were counted, and
/// one read left one pending). A leg that calls it on one way only is
/// refused, as before.
#[test]
fn a_leg_calling_its_source_on_each_way_of_an_if_is_one_send() {
    let workload = "workload { arrive batch(1); init { set prompt = 10; } send (prompt); session { turn; end; } }";
    let r = run(
        &push(
            "fork { if (prompt > 5) { P.prefill (prompt); } else { P.prefill (prompt); } } \
             D.decode (prompt) from P;",
            100,
            1.0,
        ),
        workload,
    );
    assert_eq!(r.ended, 1);
    refused(
        &push(
            "fork { if (prompt > 5) { P.prefill (prompt); } } D.decode (prompt) from P;",
            100,
            1.0,
        ),
        workload,
        "calls no engine whose requests are sent",
    );
}

/// A leg calls the same source on every way: one whose `if` calls `P` on
/// one way and `Q` on the other leaves a send the session reads from one
/// of them only, which the check could not tell apart (CodeRabbit on
/// #719). It does not link; the same source on both ways does (above).
#[test]
fn a_leg_calls_one_source_on_every_way() {
    let base = push("LEG D.decode (prompt) from P;", 100, 1.0);
    let p_start = base.find("queue P : prefill").unwrap();
    let p_end = base.find("queue D : agg").unwrap();
    let q = base[p_start..p_end]
        .replace("queue P : prefill", "queue Q : prefill")
        .replace("pool reqs on P", "pool reqs on Q")
        .replace("P (p, 1)", "Q (p, 1)");
    let d_start = p_end;
    let d_end = base.find("use \"std/kvt\";").unwrap();
    let e = base[d_start..d_end]
        .replace("queue D : agg", "queue E : agg")
        .replace("connector P", "connector Q")
        .replace("pool reqs on D", "pool reqs on E")
        .replace("from P", "from Q")
        .replace("D (p, 2)", "E (p, 2)");
    let src = |leg: &str| {
        base.replace("LEG", leg)
            .replace("use \"std/kvt\";", &format!("{q}{e}use \"std/kvt\";"))
            .replace(
                "share = share.maxmin);",
                "share = share.maxmin); kvt.nixl_push (Q, E, latency = 1, share = share.maxmin);",
            )
    };
    let workload = "workload { arrive batch(1); init { set prompt = 10; } send (prompt); session { turn; end; } }";
    refused(
        &src(
            "fork { if (prompt > 5) { P.prefill (prompt); } else { Q.prefill (prompt); } } if (prompt > 100) { E.decode (prompt) from Q; }",
        ),
        workload,
        "calls the same one on every way",
    );
    // one source on both ways links and runs
    let r = run(
        &src(
            "fork { if (prompt > 5) { P.prefill (prompt); } else { P.prefill (prompt); } } if (prompt > 100) { Q.prefill (prompt); E.decode (prompt) from Q; }",
        ),
        workload,
    );
    assert_eq!(r.ended, 1);
}
