//! A hidden attribute is the target's, not the scheduler's: the server may
//! run work by it, cache by it at release and observe it, and nothing else
//! (`docs/design/serving-specification-language.md` §2). The server's
//! session statements are checked as the parser expanded them, the engine's
//! request at its call among them.

mod common;

use serq::compile_source;

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
        workload {{ arrive batch(1); hidden o;
          init {{ set prompt = 32; set o = 4; }}
          session {{ turn; end; }} }}
        server {{ {server} }}
        "
    )
}

fn refused(server: &str, fragments: &[&str]) {
    refused_with(PREFILL, server, fragments)
}

fn refused_with(taking: &str, server: &str, fragments: &[&str]) {
    let err = compile_source(
        &common::main_source(&program_with(taking, server)),
        &common::horizon(100.0),
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
    compile_source(
        &common::main_source(&program_with(BOTH, ok)),
        &common::horizon(100.0),
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
/// requests differ (one inside the workload's `live`) was let through
/// unchecked when the check read the request the view drew (#265).
#[test]
fn the_server_is_checked_when_the_requests_differ() {
    let src = "pool live { cap 2; } stage A : fifo; stage B : fifo;
        workload { arrive poisson(1); hidden o;
          turn { set o = ~exp(10); }
          session { turn; hold live (cost(live, 1)) { turn; } } }
        server { if (o > 5) { run A (cost(A, 1)); } else { run B (cost(B, 1)); } }
        ";
    let err = compile_source(&common::main_source(src), &common::horizon(100.0)).unwrap_err();
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
               workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; } }
               session { if (o > 2) { E (prompt, 1); } else { } end; }
               ";
    let err = compile_source(&common::main_source(src), &common::horizon(100.0)).unwrap_err();
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
    compile_source(
        &common::main_source(&program_with(BOTH, after)),
        &common::horizon(100.0),
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
      workload { arrive batch(1); hidden secret; turn { set secret = 2; } }
      server {
        set a = 0; set b = 0;
        while (a == 0) { run svc (cost(svc, 1)); set a = b; set b = secret; }
        observe done = now;
      }";
    let error = compile_source(&common::main_source(source), &common::horizon(10.0)).unwrap_err();
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
        compile_source(
            &common::main_source(&program_with(request, &safe)),
            &common::horizon(100.0),
        )
        .unwrap();
    }
}

/// Joining loop paths must retain every origin, including a hidden name
/// overwritten from a different hidden input. Revealing one is not both.
#[test]
fn loop_join_preserves_origins_of_reassigned_hidden_names() {
    let src = "stage svc : delay;
      workload { arrive batch(1); hidden a, b; init { set a = 1; set b = 2; set flag = 0; } }
      server { while (1) {
        if (flag) { set a = b; }
        run svc (cost(svc, b));
        if (a > 0) {}
      } }";
    let error = compile_source(&common::main_source(src), &common::horizon(10.0)).unwrap_err();
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
         workload {{ arrive batch(1); hidden a, b; init {{ set a = 1; set b = 2; }} }}
         server {{ {body} }}"
        ))
    };
    for body in [
        "if (1) {} else { set a = b; } run svc (cost(svc, a)); if (b > 0) { observe leaked = b; }",
        "set c = 1 ? a : b; run svc (cost(svc, c)); if (b > 0) {}",
        "run svc (cost(svc, 1 || b)); if (b > 0) {}",
    ] {
        let error = compile_source(&model(body), &common::horizon(10.0)).unwrap_err();
        assert!(error.contains("`b` is hidden"), "{error}");
    }
    // Both branch arms depend on b: reading c genuinely reveals b.
    compile_source(
        &model(
            "if (1) { set c = b; } else { set c = b + 1; }
         run svc (cost(svc, c)); if (b > 0) {}",
        ),
        &common::horizon(10.0),
    )
    .unwrap();
    // Preserve dependency information after revelation so repeated runs
    // remain evidence at every loop pass, without falsely revealing b.
    compile_source(
        &model("set i = 0; while (i < 2) { run svc (cost(svc, a)); if (a > 0) {} set i = i + 1; }"),
        &common::horizon(10.0),
    )
    .unwrap();
}

#[test]
fn a_cost_family_annotation_does_not_reveal_its_index() {
    let source = "fn main() { stage svc[2] : delay;
      workload { arrive batch(1); hidden secret; turn { set secret = 0; } }
      server { run svc[0] (cost(svc[secret], 1)); if (secret > 0) {} }
    }";
    let e = compile_source(source, &common::horizon(10.0)).unwrap_err();
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
        workload {{ arrive batch(1); hidden {attribute};
          init {{ set prompt = 32; set {attribute} = 4; }}
          session {{ turn; end; }} }}
        server {{ E (prompt, {work});
          if ({attribute} > 2) {{ observe big = 1; }} else {{ observe small = 1; }} }}
        }}"
        )
    };
    let horizon = common::horizon(10.0);
    let e = compile_source(&source("E", "prompt + 0 * work(E)"), &horizon).unwrap_err();
    assert!(
        e.contains("an engine named `E` would make the one word mean two things"),
        "{e}"
    );
    let e = compile_source(&source("L", "prompt + 0 * work(E)"), &horizon).unwrap_err();
    assert!(
        e.contains("`L` is hidden from the scheduler, but the server's `if` reads it"),
        "{e}"
    );
    // a run whose work reads the attribute reveals it
    compile_source(&source("L", "prompt + 0 * L"), &horizon).unwrap();
}

/// What the workload's `turn` or `init` sets from a hidden attribute runs
/// before the request, so it is hidden as what the server sets from it is:
/// the same `x = o + 1` passed when the turn computed it and was refused
/// when the server did (#547). What the session sets after a turn reads
/// that turn's outputs, and stays visible (`examples/multi-turn/vllm.sq`).
#[test]
fn what_the_workload_sets_from_a_hidden_attribute_before_the_request_is_hidden() {
    let src = |init: &str, turn: &str, session: &str| {
        format!(
            "stage A : fifo; stage B : fifo;
            workload {{ arrive poisson(1); hidden o;
              init {{ {init} }}
              turn {{ {turn} }}
              session {{ {session} }} }}
            server {{ if (x > 5) {{ run A (cost(A, o)); }} else {{ run B (cost(B, o)); }} }}
            "
        )
    };
    let check = |s: String| compile_source(&common::main_source(&s), &common::horizon(100.0));
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
