//! A hidden attribute is the target's, not the scheduler's: the server may
//! run work by it, cache by it at release and observe it, and nothing else
//! (`docs/design/serving-specification-language.md` §2). The server's
//! session statements are checked as the parser expanded them, the engine's
//! request at its call among them.

mod common;

/// The engine's admission when it takes the prompt's blocks.
const PREFILL: &str = "kv (request.input)";

/// The engine's admission when it also leaves the whole sequence cached:
/// the output, which the server passes by the hidden `o`, is read at the
/// release.
const BOTH: &str = "kv (request.input) cache (request.input + request.output)";

/// The engine admits a request by `taking` (the prompt's blocks in most
/// tests); the server calls it.
fn program_with(taking: &str, server: &str) -> String {
    format!(
        "device gpu {{ kv cap 1000; }}
        engine E on gpu {{ tokens cap inf;
          scheduler {{ advance running; admit waiting while (running.preempted == 0) taking {taking}; }}
          worker {{ forward (1); }} }}
        pool kv on gpu {{ block 16; }}
        server (prompt, hidden o) {{ {server} }}
        "
    )
}

/// One request of a 32-token prompt that generates 4: the server's
/// `prompt` and its hidden `o`.
const WORKLOAD: &str = "workload { arrive batch(1);
  init { set prompt = 32; set o = 4; }
  send (prompt, o);
  session { turn; end; } }";

/// The deployment `src` paired with the workload file `workload`.
fn compile(src: &str, workload: &str, horizon: f64) -> Result<serq::Program, String> {
    common::compile(src, workload, &common::horizon(horizon))
}

fn refused(server: &str, fragments: &[&str]) {
    refused_with(PREFILL, server, fragments)
}

fn refused_with(taking: &str, server: &str, fragments: &[&str]) {
    let err = compile(
        &common::main_source(&program_with(taking, server)),
        WORKLOAD,
        100.0,
    )
    .unwrap_err();
    for f in fragments {
        assert!(
            err.contains(f),
            "{taking}\n{server}\n  missing {f:?} in {err}"
        );
    }
}

#[test]
fn the_server_may_run_cache_and_observe_by_a_hidden_attribute() {
    let ok = "E (prompt, o);
              observe length = o;";
    compile(
        &common::main_source(&program_with(BOTH, ok)),
        WORKLOAD,
        100.0,
    )
    .unwrap();
}

#[test]
fn the_server_may_not_decide_on_a_hidden_attribute() {
    refused(
        "if (o > 2) { E (prompt, 1); } else { observe skipped = 1; }",
        &[
            "`o` is hidden from the scheduler, but the server's `if` reads it",
            "`E (n, o);`",
        ],
    );
    refused(
        "choose j in 2 by (o); E (prompt, 1);",
        &["server's choice reads it"],
    );
}

/// The check reads the server, not what one request runs: a session whose
/// requests differ (one after the client's own `user`; it was one inside a
/// `hold` of a pool, which is the deployment's now) was let through
/// unchecked when the check read the request the view drew (#265).
#[test]
fn the_server_is_checked_when_the_requests_differ() {
    let src = "stage A : fifo; stage B : fifo;
        server (hidden o) { if (o > 5) { run A (cost(A, 1)); } else { run B (cost(B, 1)); } }
        ";
    let workload = "stage user : delay;
        workload { arrive poisson(1);
          turn { set o = ~exp(10); }
          send (o);
          session { turn; run user (cost(user, 1)); turn; } }";
    let err = compile(&common::main_source(src), workload, 100.0).unwrap_err();
    assert!(
        err.contains("`o` is hidden from the scheduler, but the server's `if` reads it"),
        "{err}"
    );
}

/// A hidden attribute passed bare to a function that takes a number is read
/// as it is in a sum: `min(o, 1)` decides on `o` as `min(o + 0, 1)` does.
#[test]
fn the_server_may_not_decide_on_a_hidden_attribute_passed_to_a_function() {
    refused(
        "if (min(o, 1) > 0) { E (prompt, 1); } else { observe skipped = 1; }",
        &["`o` is hidden from the scheduler, but the server's `if` reads it"],
    );
    refused(
        "choose j in 2 by (min(o, 1)); E (prompt, 1);",
        &["server's choice reads it"],
    );
    refused(
        "set y = min(o, 1); if (y > 0) { E (prompt, 1); } else { observe skipped = 1; }",
        &["`y` is set from the hidden `o`", "`if`"],
    );
}

/// What the server sets from a hidden attribute is hidden as well:
/// `Program::validate` sees only `long`, an attribute it may read, in the
/// engine's admission, and the input the admission reads.
#[test]
fn what_the_server_sets_from_a_hidden_attribute_is_hidden() {
    refused_with(
        "kv (long ? request.input : 16)",
        "set long = o > 2; E (prompt, 1);",
        &["`long` is set from the hidden `o` in the server, but the server's admission reads it"],
    );
    refused_with(
        PREFILL,
        "set a = o; set b = a + 1; E (b, 1);",
        &["`b` is set from the hidden `o`", "admission"],
    );
}

/// The input of a request is what its admission reads: a hidden attribute
/// there is the scheduler reading it.
#[test]
fn a_request_s_input_may_not_be_hidden() {
    refused(
        "E (o, 1);",
        &["`o` is hidden from the scheduler", "admission"],
    );
}

/// A `set` late in a loop's body reaches the body's start the next time round.
#[test]
fn a_loop_carries_what_it_sets_round_to_its_start() {
    refused(
        "set n = 0;
         loop { if (n > 3) { observe long = 1; } else { }
                E (prompt, 1);
                set n = o; }",
        &["`n` is set from the hidden `o`", "`if`"],
    );
}

/// A top-level session used to bypass the server's hidden-attribute check.
#[test]
fn a_top_level_session_cannot_bypass_the_server_check() {
    let src = "device gpu { kv cap 1000; }
               engine E on gpu { tokens cap inf;
                 scheduler { advance running; admit waiting while (running.preempted == 0) taking kv (request.input); }
                 worker { forward (1); } }
               pool kv on gpu { block 16; }
               session { if (o > 2) { E (prompt, 1); } else { } end; }
               ";
    let workload =
        "workload { arrive batch(1); init { set prompt = 32; set o = 4; } send (prompt, o); }";
    let err = compile(&common::main_source(src), workload, 100.0).unwrap_err();
    assert!(err.contains("`session` belongs inside `workload`"), "{err}");
    refused(
        "if (o > 2) { E (prompt, 1); } else { }",
        &["hidden from the scheduler"],
    );
}

/// A run whose work reads a hidden attribute reveals it when it ends (the
/// end of a decode is the EOS the scheduler sees): from there the server
/// may decide on it, and on what it set from it.
#[test]
fn a_run_reveals_what_its_work_reads() {
    let after = "set long = o > 2;
                 E (prompt, o);
                 if (long) { observe was_long = 1; } else { observe was_long = 0; }
                 if (o > 3) { observe longer = 1; } else { }";
    compile(
        &common::main_source(&program_with(BOTH, after)),
        WORKLOAD,
        100.0,
    )
    .unwrap();
    // before the request ends it is still hidden: the server may not decide
    // on it to choose what the request generates
    refused_with(
        PREFILL,
        "if (o > 2) { E (prompt, o); } else { E (prompt, 2); }",
        &[
            "`o` is hidden from the scheduler, but the server's `if` reads it before a run reveals it",
        ],
    );
}

/// Paths join conservatively: revealed on one arm only is not revealed.
#[test]
fn a_reveal_on_one_arm_only_does_not_reveal() {
    refused_with(
        PREFILL,
        "if (prompt > 10) { E (prompt, o); } else { E (prompt, 2); }
         if (o > 2) { observe long = 1; } else { }",
        &["`o` is hidden from the scheduler", "`if`"],
    );
}

/// The review's two-hop chain reaches the guard on its third evaluation.
/// Longer chains and unconditional loops need the same fixed-point check.
#[test]
fn hidden_values_propagate_through_every_loop_pass() {
    let source = "stage svc : delay;
      server (hidden secret) {
        set a = 0; set b = 0;
        while (a == 0) { run svc (cost(svc, 1)); set a = b; set b = secret; }
        observe done = now;
      }";
    let workload = "workload { arrive batch(1); turn { set secret = 2; } send (secret); }";
    let error = compile(&common::main_source(source), workload, 10.0).unwrap_err();
    assert!(
        error.contains("`a` is set from the hidden `secret`"),
        "{error}"
    );
    assert!(error.contains("server's while reads it"), "{error}");

    // the request generates `o` tokens to reveal it, 1 not to
    let request = PREFILL;
    for depth in [3, 8] {
        let init = (0..depth)
            .map(|i| format!("set x{i} = 0; "))
            .collect::<String>();
        let mut chain = (0..depth - 1)
            .map(|i| format!("set x{i} = x{}; ", i + 1))
            .collect::<String>();
        chain.push_str(&format!("set x{} = o;", depth - 1));
        let work = "E (prompt, 1);";
        refused_with(
            request,
            &format!("{init} while (x0 == 0) {{ {work} {chain} }}"),
            &["`x0` is set from the hidden `o`", "server's while reads it"],
        );
        refused_with(
            request,
            &format!("{init} loop {{ if (x0 == 0) {{ observe flag = 1; }} {work} {chain} }}"),
            &["`x0` is set from the hidden `o`", "server's `if` reads it"],
        );
        // A run revealing o before the decision makes the same chain legal.
        let safe = format!(
            "E (prompt, o);
          {init} while (x0 == 0) {{ {work} {chain} }}"
        );
        compile(
            &common::main_source(&program_with(request, &safe)),
            WORKLOAD,
            100.0,
        )
        .unwrap();
    }
}

/// Joining loop paths must retain every origin, including a hidden name
/// overwritten from a different hidden input. Revealing one is not both.
#[test]
fn loop_join_preserves_origins_of_reassigned_hidden_names() {
    let src = "stage svc : delay;
      server (hidden a, hidden b, flag) { while (1) {
        if (flag) { set a = b; }
        run svc (cost(svc, b));
        if (a > 0) {}
      } }";
    let workload = "workload { arrive batch(1); init { set a = 1; set b = 2; set flag = 0; } send (a, b, flag); }";
    let error = compile(&common::main_source(src), workload, 10.0).unwrap_err();
    // The else path leaves a's original hidden value. Running by b cannot
    // reveal it just because the other path assigned a from b.
    assert!(error.contains("hidden `a`"), "{error}");
    assert!(error.contains("server's `if` reads it"), "{error}");
}

#[test]
fn a_run_reveals_only_origins_present_on_every_path() {
    let model = |body: &str| {
        common::main_source(&format!(
            "stage svc : delay;
         server (hidden a, hidden b) {{ {body} }}"
        ))
    };
    let w = "workload { arrive batch(1); init { set a = 1; set b = 2; } send (a, b); }";
    for body in [
        "if (1) {} else { set a = b; } run svc (cost(svc, a)); if (b > 0) { observe leaked = b; }",
        "set c = 1 ? a : b; run svc (cost(svc, c)); if (b > 0) {}",
        "run svc (cost(svc, 1 || b)); if (b > 0) {}",
    ] {
        let error = compile(&model(body), w, 10.0).unwrap_err();
        assert!(error.contains("`b` is hidden"), "{error}");
    }
    // Both branch arms depend on b: reading c genuinely reveals b.
    compile(
        &model(
            "if (1) { set c = b; } else { set c = b + 1; }
         run svc (cost(svc, c)); if (b > 0) {}",
        ),
        w,
        10.0,
    )
    .unwrap();
    // Preserve dependency information after revelation so repeated runs
    // remain evidence at every loop pass, without falsely revealing b.
    compile(
        &model("set i = 0; while (i < 2) { run svc (cost(svc, a)); if (a > 0) {} set i = i + 1; }"),
        w,
        10.0,
    )
    .unwrap();
}

#[test]
fn a_cost_family_annotation_does_not_reveal_its_index() {
    let source = "fn main() { stage svc[2] : delay;
      server (hidden secret) { run svc[0] (cost(svc[secret], 1)); if (secret > 0) {} }
    }";
    let workload = "workload { arrive batch(1); turn { set secret = 0; } send (secret); }";
    let e = compile(source, workload, 10.0).unwrap_err();
    assert!(e.contains("hidden"), "{e}");
}

/// A bare name reveals its attribute only where the function takes a
/// number (#467): `work(E)` reads the engine `E`, so a run by it did not
/// reveal a hidden attribute `E`. Since #470 no attribute may take the
/// engine's name, and a run reveals a hidden attribute only where its work
/// reads it.
#[test]
fn a_stage_passed_bare_does_not_reveal_an_attribute_of_its_name() {
    let source = |attribute: &str, work: &str| {
        format!(
            "fn main() {{
        device gpu {{ kv cap 1000; }}
        engine E on gpu {{ tokens cap inf;
          scheduler {{ advance running; admit waiting while (running.preempted == 0) taking kv (request.input); }}
          worker {{ forward (1); }} }}
        pool kv on gpu {{ block 16; }}
        server (prompt, hidden {attribute}) {{ E (prompt, {work});
          if ({attribute} > 2) {{ observe big = 1; }} else {{ observe small = 1; }} }}
        }}"
        )
    };
    let horizon = |attribute: &str| {
        let workload = format!(
            "workload {{ arrive batch(1);
               init {{ set prompt = 32; set {attribute} = 4; }}
               send (prompt, {attribute});
               session {{ turn; end; }} }}"
        );
        workload
    };
    let e = compile(&source("E", "prompt + 0 * work(E)"), &horizon("E"), 10.0).unwrap_err();
    assert!(
        e.contains("an engine named `E` would make the one word mean two things"),
        "{e}"
    );
    let e = compile(&source("L", "prompt + 0 * work(E)"), &horizon("L"), 10.0).unwrap_err();
    assert!(
        e.contains("`L` is hidden from the scheduler, but the server's `if` reads it"),
        "{e}"
    );
    // a run whose work reads the attribute reveals it
    compile(&source("L", "prompt + 0 * L"), &horizon("L"), 10.0).unwrap();
}

/// What the workload's `turn` or `init` sets from a hidden attribute runs
/// before the request, so it is hidden as what the server sets from it is:
/// the same `x = o + 1` passed when the turn computed it and was refused
/// when the server did (#547). What the session sets after a turn reads
/// that turn's outputs, and stays visible (`examples/multi-turn/vllm.sq`).
#[test]
fn what_the_workload_sets_from_a_hidden_attribute_before_the_request_is_hidden() {
    let deployment = "stage A : fifo; stage B : fifo;
            server (x, hidden o) { if (x > 5) { run A (cost(A, o)); } else { run B (cost(B, o)); } }
            ";
    let src = |init: &str, turn: &str, session: &str| {
        format!(
            "workload {{ arrive poisson(1);
              init {{ {init} }}
              turn {{ {turn} }}
              send (x, o);
              session {{ {session} }} }}"
        )
    };
    let check = |w: String| compile(&common::main_source(deployment), &w, 100.0);
    let err = check(src(
        "set x = 0;",
        "set o = ~exp(10); set x = o + 1;",
        "turn;",
    ))
    .unwrap_err();
    assert!(
        err.contains(
            "`x` is set from the hidden `o` in the workload's turn, but the server's `if` reads it"
        ),
        "{err}"
    );
    let err = check(src("set o = 4; set x = o;", "set y = 1;", "turn;")).unwrap_err();
    assert!(
        err.contains("`x` is set from the hidden `o` in the workload's init"),
        "{err}"
    );
    // so is what the session sets before the request, from `init` or from
    // a hidden attribute it draws again
    let err = check(src("set o = ~exp(10);", "set y = 1;", "set x = o; turn;")).unwrap_err();
    assert!(
        err.contains("`x` is set from the hidden `o` in the workload's session"),
        "{err}"
    );
    let err = check(src(
        "set x = 0; set o = 1; set more = 1;",
        "set y = 1;",
        "turn; while (more) { set o = ~exp(10); set x = o; set more = 0; turn; }",
    ))
    .unwrap_err();
    assert!(
        err.contains("`x` is set from the hidden `o` in the workload's session"),
        "{err}"
    );
    // a later `set` in the turn of a visible value makes it visible again
    check(src("set o = 4; set x = o;", "set x = 1;", "turn;")).unwrap();
    // the session reads the turn that has ended
    check(src(
        "set x = 0; set o = 0;",
        "set o = ~exp(10);",
        "turn; set x = x + o; turn;",
    ))
    .unwrap();
}

/// The issue's workload (#554): one hidden draw per turn, sent as it is.
const DRAW: &str = "workload { arrive poisson(1); turn { Size o = ~exp(10); } send (o); }";

/// A pool `kv` with `clause`, a stage `A` (and `B` when the server runs
/// it), and a server that takes the hidden `o` and runs `server`.
fn pool_program(clause: &str, server: &str) -> String {
    let b = if server.contains("run B") {
        "stage B : fifo;"
    } else {
        ""
    };
    common::main_source(&format!(
        "pool kv {{ cap 30; {clause} }}
         stage A : fifo; {b}
         server (hidden o) {{ {server} }}"
    ))
}

fn key_refused(src: &str, workload: &str, expected: &str) {
    let err = compile(src, workload, 100.0).unwrap_err();
    assert!(
        err.contains(expected),
        "{src}\n  missing {expected:?} in {err}"
    );
}

/// The bug as found (#554 q2): `queue by (o)` was refused and `set y = o;`
/// with `queue by (y)` linked, since `Program::validate` sees only the
/// hidden attribute and the linker, which follows what is set from it,
/// checked the server's statements and not the keys. Each key reads `y`
/// while `A`, whose work reveals `o`, has not ended: at the hold, during
/// the run in the hold, and wherever the entry's session is. The two
/// rules are held equal: the same key on `o` gives validate's message.
#[test]
fn a_pool_key_may_not_read_what_the_server_set_from_a_hidden_attribute() {
    use serq::ir::Moment;
    let server = "set y = o; hold kv (cost(kv, 1)) { run A (cost(A, o)); }";
    for (clause, moment) in [
        ("queue by (y);", Moment::Select),
        ("evict by (y);", Moment::Evict),
        ("preempt by (y);", Moment::Victim),
    ] {
        key_refused(
            &pool_program(clause, server),
            DRAW,
            &format!(
                "pool `kv`: `y` is set from the hidden `o` in the server, but is read in {moment}"
            ),
        );
        key_refused(
            &pool_program(&clause.replace("(y)", "(o)"), server),
            DRAW,
            &format!("pool `kv`: `o` is hidden from the scheduler, but is read in {moment}"),
        );
    }
    // a spill clause is read for an entry, as an eviction key is
    key_refused(
        &common::main_source(
            "pool l2 { cap 100; }
             stage copy : delay;
             pool kv { cap 30; spill l2 via copy (1) when (y > 1); }
             stage A : fifo;
             server (hidden o) { set y = o; hold kv (cost(kv, 1)) { run A (cost(A, o)); } }",
        ),
        DRAW,
        "pool `kv`: `y` is set from the hidden `o` in the server, but is read in an eviction key \
         or spill clause",
    );
}

/// The same, set in the workload's `turn` and sent (#554 q1): the message
/// names where it was set.
#[test]
fn a_pool_key_may_not_read_what_the_turn_set_from_a_hidden_attribute() {
    let src = common::main_source(
        "pool kv { cap 30; queue by (x); }
         stage A : fifo;
         server (x, hidden o) { hold kv (cost(kv, 1)) { run A (cost(A, o)); } }",
    );
    key_refused(
        &src,
        "workload { arrive poisson(1); turn { Size o = ~exp(10); set x = o; } send (x, o); }",
        "pool `kv`: `x` is set from the hidden `o` in the workload's turn, but is read in a \
         pool's queue keys, read before selecting a waiting session",
    );
    // and bound by `send`, which sets it at the turn's end
    key_refused(
        &src,
        "workload { arrive poisson(1); turn { Size o = ~exp(10); } send (x: o, o); }",
        "`x` is set from the hidden `o` in the workload's turn, but is read in a pool's queue keys",
    );
}

/// `program_with`'s engine with `scheduler` and `pool` as written.
fn engine_program(scheduler: &str, pool: &str, server: &str) -> String {
    common::main_source(&format!(
        "device gpu {{ kv cap 1000; }}
        engine E on gpu {{ tokens cap inf;
          scheduler {{ {scheduler} }}
          worker {{ forward (1); }} }}
        pool kv on gpu {{ block 16; {pool} }}
        server (prompt, hidden o) {{ {server} }}
        "
    ))
}

/// An engine reads its running requests' keys and `only`s, and its pool's
/// preempt keys, while the request runs, before its end reveals the output.
#[test]
fn an_engine_s_keys_may_not_read_what_the_server_set_from_a_hidden_attribute() {
    use serq::ir::Moment;
    let admit = "while (running.preempted == 0) taking kv (request.input);";
    let server = "set y = o; E (prompt, o);";
    for scheduler in [
        format!("advance running by (y); admit waiting {admit}"),
        format!("advance running only (y > 0); admit waiting {admit}"),
        format!("advance running; admit waiting only (y > 0) {admit}"),
    ] {
        key_refused(
            &engine_program(&scheduler, "", server),
            WORKLOAD,
            &format!(
                "engine `E`: `y` is set from the hidden `o` in the server, but is read in {}",
                Moment::Serve
            ),
        );
    }
    key_refused(
        &engine_program(
            &format!("advance running; admit waiting {admit}"),
            "preempt by (y);",
            server,
        ),
        WORKLOAD,
        &format!(
            "pool `kv`: `y` is set from the hidden `o` in the server, but is read in {}",
            Moment::Victim
        ),
    );
}

/// A connector's `load` is read when the scheduler admits the request, as
/// `taking` is: the linker's list of what an admission reads lacked it.
#[test]
fn a_connector_s_load_is_its_engine_s_admission() {
    let src = |server: &str| {
        common::main_source(&format!(
            "queue lmc : l2 {{ pool kv {{ cap 1000; }} serve ps(1); }}
             device gpu {{ kv cap 1000; }}
             pool kv on gpu {{ }}
             engine E on gpu {{
               tokens cap inf;
               connector lmc {{ per_token (1e-6); load (min(y, request.input)); }}
               scheduler {{ advance running; admit waiting while (running.preempted == 0) taking kv (request.known); }}
               worker {{ forward (1); }}
             }}
             server (prompt, hidden o) {{ {server} }}"
        ))
    };
    key_refused(
        &src("set y = o; E (prompt, o);"),
        WORKLOAD,
        "`y` is set from the hidden `o` in the server, but the server's admission reads it",
    );
    compile(
        &src("set y = 0; E (prompt, o); set y = o; E (prompt, 1);"),
        WORKLOAD,
        100.0,
    )
    .unwrap();
}

/// A lease outlives the hold that took it: its keys read the session it is
/// of wherever the session is, here in a run that has not revealed `o`.
#[test]
fn a_lease_s_keys_read_the_lessee_wherever_it_is() {
    use serq::ir::Moment;
    for clause in [
        "preempt leases by (y);",
        "preempt leases by (1) every (1) suspend by (y);",
    ] {
        key_refused(
            &pool_program(
                clause,
                "hold kv (cost(kv, 1)) { } lease kv (inf);
                 set y = o; run A (cost(A, o));",
            ),
            DRAW,
            &format!(
                "pool `kv`: `y` is set from the hidden `o` in the server, but is read in {}",
                Moment::Victim
            ),
        );
        // set once the run has revealed `o`, it is the scheduler's
        compile(
            &pool_program(
                clause,
                "hold kv (cost(kv, 1)) { } lease kv (inf);
                 run A (cost(A, o)); set y = o;",
            ),
            DRAW,
            100.0,
        )
        .unwrap();
    }
}

/// An eviction key reads the session that wrote the entry, wherever it is:
/// after the request, with what the server set, and in the workload's tool
/// call, with what the workload set for its next turn.
#[test]
fn an_eviction_key_reads_the_session_after_its_request() {
    key_refused(
        &pool_program(
            "evict by (y);",
            "hold kv (cost(kv, 1)) { run A (cost(A, 1)); } set y = o;",
        ),
        DRAW,
        "`y` is set from the hidden `o` in the server, but is read in an eviction key",
    );
    let src = common::main_source(
        "pool kv { cap 30; evict by (x); }
         stage A : fifo;
         server (x, hidden o) { hold kv (cost(kv, 1)) { run A (cost(A, o)); } }",
    );
    let workload = |session: &str| {
        format!(
            "workload {{ arrive batch(1);
               turn {{ set o = ~exp(10); set x = 1; }}
               send (x, o);
               session {{ {session} }} }}"
        )
    };
    key_refused(
        &src,
        &workload("turn; set o = ~exp(10); set x = o; tool (5); turn; end;"),
        "`x` is set from the hidden `o` in the workload's session, but is read in an eviction key",
    );
    // without the tool call nothing reads `x` before the turn sets it again
    compile(
        &src,
        &workload("turn; set o = ~exp(10); set x = o; turn; end;"),
        100.0,
    )
    .unwrap();
}

/// The workload's statements that free its own pool, a `release`, a
/// `shrink` and a hold's end, admit what waits (`Interp::try_admit_all`),
/// and an admission may evict the deployment's entry this session wrote,
/// whose key reads it there: the workload's walk gathered only its waits.
#[test]
fn an_eviction_key_reads_the_session_where_the_workload_frees_a_pool() {
    let src = common::main_source(
        "pool kv { cap 30; evict by (x); }
         stage A : fifo;
         server (x, hidden o) { hold kv (cost(kv, 1)) { run A (cost(A, o)); } }",
    );
    let workload = |session: &str| {
        format!(
            "pool c {{ cap 10; }}
             workload {{ arrive batch(1);
               turn {{ set o = ~exp(10); set x = 1; }}
               send (x, o);
               session {{ turn; {session} turn; end; }} }}"
        )
    };
    for session in [
        "hold c (cost(c, 1)) { } lease c (inf); set o = ~exp(10); set x = o; release c; set x = 1;",
        "hold c (cost(c, 1)) { set o = ~exp(10); set x = o; } set x = 1;",
        "hold c (cost(c, 2)) { set o = ~exp(10); set x = o; shrink c (cost(c, 1)); set x = 1; }",
    ] {
        key_refused(
            &src,
            &workload(session),
            "`x` is set from the hidden `o` in the workload's session, but is read in an \
             eviction key",
        );
    }
    // set again before the pool is freed, nothing reads it
    compile(
        &src,
        &workload(
            "hold c (cost(c, 1)) { } lease c (inf); set o = ~exp(10); set x = o; set x = 1; \
             release c;",
        ),
        100.0,
    )
    .unwrap();
    // the server's `shrink` frees units as its `release` does
    key_refused(
        &pool_program(
            "evict by (y);",
            "set y = 1; hold kv (cost(kv, 2)) { set y = o; shrink kv (cost(kv, 1)); set y = 1; \
             run A (cost(A, o)); }",
        ),
        DRAW,
        "`y` is set from the hidden `o` in the server, but is read in an eviction key",
    );
}

/// A preempted hold goes back to its queue with the attributes it has then
/// (`Interp::preempt`): under `requeue tail` as a newcomer, selected by the
/// queue's keys again. Under `requeue head` (and `preempt lifo`) it goes
/// back ahead of the keys, which never read it (`next_waiter_among`), and
/// a pool that does not preempt never puts it back: neither is refused.
#[test]
fn a_preempted_hold_is_selected_again_by_its_queue_keys() {
    let server = "set y = 1; hold kv (cost(kv, 1)) { set y = o; run A (cost(A, o)); }";
    key_refused(
        &pool_program("queue by (y); preempt by (1) requeue tail;", server),
        DRAW,
        "`y` is set from the hidden `o` in the server, but is read in a pool's queue keys",
    );
    for clause in [
        "queue by (y);",
        "queue by (y); preempt by (1);",
        "queue by (y); preempt lifo;",
    ] {
        compile(&pool_program(clause, server), DRAW, 100.0).unwrap();
    }
}

/// A preempted hold's header is evaluated again when it goes back to the
/// queue and at each admission it tries (`Interp::wanted`, `pending_now`),
/// on the attributes as they are then, whether it re-enters at the head or
/// the tail: a copy its body set before a run revealed it is read there.
#[test]
fn a_preempted_hold_s_header_is_read_again() {
    let server = "set y = 1; hold kv (cost(kv, y)) { set y = o; run A (cost(A, o)); }";
    for clause in ["preempt by (1);", "preempt by (1) requeue tail;"] {
        key_refused(
            &pool_program(clause, server),
            DRAW,
            "`y` is set from the hidden `o` in the server, but the server's admission of a \
             preempted hold reads it before a run reveals it",
        );
    }
    // a pool that does not preempt reads the header once, at the hold
    compile(&pool_program("", server), DRAW, 100.0).unwrap();
    // set once `A` has revealed `o`, the copy is the scheduler's
    compile(
        &pool_program(
            "preempt by (1);",
            "set y = 1; hold kv (cost(kv, y)) { run A (cost(A, o)); set y = o; run A (cost(A, 1)); }",
        ),
        DRAW,
        100.0,
    )
    .unwrap();
}

/// A lease's time is read at the hold's end, where the lease starts
/// (`Interp::end_hold`), on the attributes as they are there.
#[test]
fn a_lease_s_time_is_read_at_the_hold_s_end() {
    key_refused(
        &pool_program(
            "",
            "set y = 1; hold kv (cost(kv, 1)) { run A (cost(A, 1)); set y = o; } lease kv (y);",
        ),
        DRAW,
        "`y` is set from the hidden `o` in the server, but the server's lease reads it before a \
         run reveals it",
    );
    compile(
        &pool_program(
            "",
            "set y = 1; hold kv (cost(kv, 1)) { set y = o; run A (cost(A, o)); } lease kv (y);",
        ),
        DRAW,
        100.0,
    )
    .unwrap();
}

/// A key reads the session where the scheduler reads it: what a run has
/// revealed by then it may read.
#[test]
fn a_key_may_read_what_a_run_has_revealed() {
    let revealed = "run A (cost(A, o)); set y = o; hold kv (cost(kv, 1)) { run B (cost(B, 1)); }";
    for clause in ["queue by (y);", "preempt by (y);", "evict by (y);"] {
        compile(&pool_program(clause, revealed), DRAW, 100.0).unwrap();
    }
    // the queue reads `y` at the hold, after `A` has revealed `o`; an
    // eviction key reads it during `A` too, before: in the second turn the
    // entry the first turn's hold wrote is read while `A` runs
    // (`Interp::node_writer`: the live session's attributes)
    let during = "set y = o; run A (cost(A, o)); hold kv (cost(kv, 1)) { run B (cost(B, 1)); }";
    compile(&pool_program("queue by (y);", during), DRAW, 100.0).unwrap();
    key_refused(
        &pool_program("evict by (y);", during),
        "workload { arrive poisson(1); turn { Size o = ~exp(10); } send (o);
           session { turn; turn; end; } }",
        "`y` is set from the hidden `o` in the server, but is read in an eviction key",
    );
    // an engine's request reveals its output when it ends
    let admit = "admit waiting while (running.preempted == 0) taking kv (request.input);";
    compile(
        &engine_program(
            &format!("advance running by (y); {admit}"),
            "",
            "E (prompt, o); set y = o; E (prompt, 1);",
        ),
        WORKLOAD,
        100.0,
    )
    .unwrap();
}
