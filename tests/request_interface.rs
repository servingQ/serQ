//! The server's signature (`docs/design/request-interface.md`): what a
//! request carries, `server (P, …)`, and what its reply carries, `reply (R,
//! …)`. The workload binds the parameters with `send (P: e, …);` and reads
//! the reply as `reply.R`; nothing else crosses. A bare `server { … }`
//! takes nothing and replies nothing.

mod common;

use serq::{compile_source, run_source};

fn compile(body: &str) -> Result<serq::Program, String> {
    compile_source(&common::main_source(body), &common::horizon(50.0))
}

fn refused(body: &str, fragments: &[&str]) {
    let err = compile(body).unwrap_err();
    for f in fragments {
        assert!(err.contains(f), "{body}\n  missing {f:?} in {err}");
    }
}

/// A queue, a workload of exponential jobs and a counter of turns.
fn with(workload: &str, server: &str) -> String {
    format!(
        "stage svc : fifo;
        workload {{ arrive poisson(1); init {{ set K = 0; }} turn {{ set s = ~exp(1); }}
          {workload} }}
        {server}"
    )
}

#[test]
fn a_request_carries_what_the_workload_sends() {
    compile(&with(
        "send (work: s + K); session { turn; set K = K + 1; }",
        "server (work) { run svc (cost(svc, work)); }",
    ))
    .unwrap();
}

#[test]
fn the_server_reads_no_attribute_the_workload_keeps() {
    refused(
        &with(
            "send (work: s);",
            "server (work) { run svc (cost(svc, work + K)); }",
        ),
        &[
            "the server reads `K`, which the workload sets",
            "a request carries `work`",
            "bind it in the workload's `send (…)`",
        ],
    );
}

#[test]
fn the_scheduler_reads_what_the_server_reads() {
    let pool = |key: &str| {
        format!(
            "pool q {{ cap 1; queue by ({key}); }}
            {}",
            with(
                "send (work: s);",
                "server (work) { hold q (cost(q, 1)) { run svc (cost(svc, work)); } }",
            )
        )
    };
    compile(&pool("work")).unwrap();
    refused(
        &pool("s"),
        &["the server reads `s`, which the workload sets"],
    );
}

#[test]
fn the_workload_reads_the_reply_and_nothing_else_of_the_server() {
    let server = "server (work) reply (done) { run svc (cost(svc, work)); set done = now; }";
    compile(&with(
        "send (work: s); session { turn; observe at = reply.done; }",
        server,
    ))
    .unwrap();
    refused(
        &with(
            "send (work: s); session { turn; observe at = done; }",
            server,
        ),
        &[
            "the workload reads `done`, which the server sets",
            "read the reply: `reply.done`",
        ],
    );
    refused(
        &with(
            "send (work: s); session { turn; observe at = reply.gone; }",
            server,
        ),
        &["`reply.gone`: the server's reply carries `done`"],
    );
}

#[test]
fn a_reply_is_read_in_the_session() {
    refused(
        &with(
            "send (work: s);",
            "server (work) reply (done) { run svc (cost(svc, reply.done)); set done = 1; }",
        ),
        &["`reply.done` is read in the workload's `session`, after `turn;`"],
    );
}

#[test]
fn the_server_sets_its_reply_on_every_path_that_completes() {
    refused(
        &with(
            "send (work: s);",
            "server (work) reply (done) { if (work > 1) { set done = 1; } run svc (cost(svc, work)); }",
        ),
        &["the reply carries `done`, and the server can complete a request without setting it"],
    );
    // a path that ends the session completes no request
    compile(&with(
        "send (work: s);",
        "server (work) reply (done) {
           if (work > 5) { end; } else { set done = 1; }
           run svc (cost(svc, work)); }",
    ))
    .unwrap();
}

#[test]
fn every_parameter_is_sent() {
    refused(
        &with("", "server (work) { run svc (cost(svc, work)); }"),
        &["the server takes `work`, and the workload does not send it"],
    );
    refused(
        &with(
            "send (work: s, other: 1);",
            "server (work) { run svc (cost(svc, work)); }",
        ),
        &["`send` binds `other`, which the server does not take; it takes `work`"],
    );
    // no program needs a default yet (CLAUDE.md, a construct some program uses)
    refused(
        &with(
            "send (work: s);",
            "server (work, factor = 2) { run svc (cost(svc, work * factor)); }",
        ),
        &["`factor = …`: a parameter has no default; the workload sends it"],
    );
}

/// The workload reads its own attributes, what it sends as it is and the
/// reply: not the server's parameter names, nor what an engine marks.
#[test]
fn the_workload_reads_its_own() {
    refused(
        &with(
            "send (work: s); session { turn; observe x = work; }",
            "server (work) { run svc (cost(svc, work)); }",
        ),
        &["the workload reads `work`, the server's parameter"],
    );
    compile(&with(
        "send (s); session { turn; observe x = s; }",
        "server (s) { run svc (cost(svc, s)); }",
    ))
    .unwrap();
    let engine = "device gpu { kv cap 1000; }
        engine e on gpu { tokens cap inf;
          scheduler { advance running; admit waiting while (running.preempted == 0) taking kv (request.input); }
          worker { forward (1); } }
        pool kv on gpu { block 16; }";
    refused(
        &format!(
            "{engine}
            {}",
            with(
                "send (work: ceil(s * 10) + 1); session { turn; observe x = e.first; }",
                "server (work) { e (work, 1); }",
            )
        ),
        &[
            "the workload reads `e.first`, which the server sets",
            "(`set x = e.first;`), add `x` to `reply (…)`",
        ],
    );
}

/// A queue's entry reads what it is passed and the hidden parameters, as it
/// read the hidden attributes; the help names the parameter's `hidden`.
#[test]
fn an_entry_reads_the_hidden_parameters() {
    let program = |server: &str| {
        format!(
            "queue gw : gateway {{ route {{ E.decode (prompt); }} }}
             queue E : decode {{ device gpu {{ kv cap 100; }} pool kv on gpu {{ }}
               engine on gpu {{ tokens cap inf; scheduler {{ advance running; admit waiting while (running.preempted == 0); }} worker {{ forward (1); }} }}
               decode (prompt) {{ hold kv (cost(kv, prompt)) {{
                 run E prefill (cost(E, 1)) growing kv; run E decode (cost(E, o - 1)) growing kv; }} }} }}
             workload {{ arrive batch(1); init {{ set p = 32; set out = 4; }}
               send (prompt: p, o: out); session {{ turn; end; }} }}
             server ({server}) {{ gw.route(); }}"
        )
    };
    compile(&program("prompt, hidden o")).unwrap();
    refused(
        &program("prompt, o"),
        &["take `o` `hidden` in `server (…, hidden o)`"],
    );
}

#[test]
fn a_reply_is_read_after_a_turn() {
    refused(
        &with(
            "send (work: s); session { observe x = reply.done; turn; }",
            "server (work) reply (done) { run svc (cost(svc, work)); set done = 1; }",
        ),
        &["`reply.done` is read in the workload's `session`, after `turn;`"],
    );
}

#[test]
fn a_parameter_named_as_a_workload_attribute_is_that_attribute() {
    compile(&with("send (s);", "server (s) { run svc (cost(svc, s)); }")).unwrap();
    refused(
        &with("send (s: s * 2);", "server (s) { run svc (cost(svc, s)); }"),
        &["the workload sets `s`, and `send` binds the parameter `s` to another value"],
    );
    refused(
        &with("send (t);", "server (t) { run svc (cost(svc, t)); }"),
        &["`send (t)` sends the workload's `t`, which nothing sets"],
    );
    // one spelling: `send (s)`, not `send (s: s)`
    refused(
        &with("send (s: s);", "server (s) { run svc (cost(svc, s)); }"),
        &["`s: s` sends `s` as it is: write `send (s)`"],
    );
}

#[test]
fn hiding_is_a_parameters_under_a_signature() {
    refused(
        &with(
            "hidden s; send (s);",
            "server (s) { run svc (cost(svc, s)); }",
        ),
        &[
            "`hidden s;` in `workload`: the server hides a parameter in its signature",
            "`server (…, hidden s, …)`",
        ],
    );
    // a hidden parameter is hidden from the scheduler as `hidden` was
    let key = "pool q { cap 1; queue by (work); }";
    refused(
        &format!(
            "{key}
            {}",
            with(
                "send (work: s);",
                "server (hidden work) { hold q (cost(q, 1)) { run svc (cost(svc, work)); } }",
            )
        ),
        &["pool `q`: `work` is hidden from the scheduler"],
    );
}

#[test]
fn a_bare_server_takes_nothing() {
    refused(
        &with("", "server { run svc (cost(svc, s)); }"),
        &["the server reads `s`, which the workload sets; a request carries nothing"],
    );
    refused(
        &with("send (s);", "server { run svc (cost(svc, s)); }"),
        &["`send` binds `s`, which the server does not take; it takes nothing"],
    );
    refused(
        &with(
            "session { turn; observe x = reply.done; }",
            "server { run svc (cost(svc, 1)); set done = 1; }",
        ),
        &["`reply.done`: the server's reply carries nothing"],
    );
    compile(&with("", "server { run svc (cost(svc, 1)); }")).unwrap();
    // one spelling of nothing
    refused(
        &with("", "server () { run svc (cost(svc, 1)); }"),
        &["`server ()` takes nothing, as `server { … }` does"],
    );
    refused(
        &with("send ();", "server { run svc (cost(svc, 1)); }"),
        &["`send ()` sends nothing: leave it out"],
    );
}

/// A parameter sent as the workload's attribute of the same name is that
/// attribute: the `turn` block keeps only its draw.
#[test]
fn sending_an_attribute_as_it_is_adds_nothing_to_the_ir() {
    let turn = |workload: &str, server: &str| {
        let p = compile(&with(workload, server)).unwrap();
        // the hidden parameter, and nothing else, is hidden
        assert_eq!(p.hidden.len(), 1);
        p.blocks[p.turn].len()
    };
    assert_eq!(
        turn("send (s);", "server (hidden s) { run svc (cost(svc, s)); }"),
        1
    );
    assert_eq!(
        turn(
            "send (w: s);",
            "server (hidden w) { run svc (cost(svc, w)); }"
        ),
        2
    );
}

/// A parameter bound to an expression is set at the turn, from the turn's
/// draws: the run is the one of a server reading the expression of
/// parameters sent as they are.
#[test]
fn a_sent_expression_is_its_value_at_the_turn() {
    let session = "session { turn; while (K < 3) { set K = K + 1; turn; } }";
    let as_is = with(
        &format!("send (s, K); {session}"),
        "server (s, K) { run svc (cost(svc, s + K)); observe w = s + K; }",
    );
    let bound = with(
        &format!("send (work: s + K); {session}"),
        "server (work) { run svc (cost(svc, work)); observe w = work; }",
    );
    let run = |src: &str| {
        run_source(&common::main_source(src), &common::horizon(200.0), None)
            .unwrap()
            .json()
    };
    assert_eq!(run(&as_is), run(&bound));
}
