//! A hidden attribute is the target's, not the scheduler's: the server may
//! run work by it, cache by it at release and observe it, and nothing else
//! (`docs/design/serving-specification-language.md` §2). The server's
//! session statements are checked as the parser expanded them.

mod common;

use serq::compile_source;

fn program(server: &str) -> String {
    format!(
        "pool kv {{ cap 1000; block 16; }}
        stage E : step {{ cost 1; memory kv; }}
        workload {{ arrive batch(1); hidden o;
          init {{ set prompt = 32; set o = 4; }}
          session {{ turn; end; }} }}
        server {{ {server} }}
        "
    )
}

fn refused(server: &str, fragments: &[&str]) {
    let err = compile_source(
        &common::main_source(&program(server)),
        &common::horizon(100.0),
    )
    .unwrap_err();
    for f in fragments {
        assert!(err.contains(f), "{server}\n  missing {f:?} in {err}");
    }
}

#[test]
fn the_server_may_run_cache_and_observe_by_a_hidden_attribute() {
    let ok = "hold kv (cost(kv, prompt)) { run E prefill (cost(E, prompt)) growing kv; run E decode (cost(E, o - 1)) growing kv; } cache (cost(kv, prompt + o));
              observe length = o;";
    compile_source(&common::main_source(&program(ok)), &common::horizon(100.0)).unwrap();
}

#[test]
fn the_server_may_not_decide_on_a_hidden_attribute() {
    refused(
        "branch (o > 2) { hold kv (cost(kv, prompt)) { run E prefill (cost(E, prompt)) growing kv; } } else { observe skipped = 1; }",
        &[
            "`o` is hidden from the scheduler, but the server's branch reads it",
            "run E decode (cost(E, o))",
        ],
    );
    refused(
        "choose j in 2 by (o); hold kv (cost(kv, prompt)) { run E prefill (cost(E, prompt)) growing kv; }",
        &["server's choice reads it"],
    );
}

/// What the server sets from a hidden attribute is hidden as well:
/// `Program::validate` sees only `long`, an attribute it may read.
#[test]
fn what_the_server_sets_from_a_hidden_attribute_is_hidden() {
    refused(
        "set long = o > 2; hold kv (cost(kv, long ? prompt : 16)) { run E prefill (cost(E, prompt)) growing kv; }",
        &["`long` is set from the hidden `o` in the server, but the server's admission reads it"],
    );
    refused(
        "set a = o; set b = a + 1; hold kv (cost(kv, prompt)) { run E prefill (cost(E, prompt)) growing kv; grow kv (cost(kv, b)); }",
        &["`b` is set from the hidden `o`", "allocation"],
    );
}

/// A `set` late in a loop's body reaches the body's start the next time round.
#[test]
fn a_loop_carries_what_it_sets_round_to_its_start() {
    refused(
        "set n = 0;
         loop { branch (n > 3) { observe long = 1; } else { }
                hold kv (cost(kv, prompt)) { run E prefill (cost(E, prompt)) growing kv; }
                set n = o; }",
        &["`n` is set from the hidden `o`", "branch"],
    );
}

/// A top-level session used to bypass the server's hidden-attribute check.
#[test]
fn a_top_level_session_cannot_bypass_the_server_check() {
    let src = "pool kv { cap 1000; block 16; }
               stage E : step { cost 1; memory kv; }
               workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; } }
               session { branch (o > 2) { hold kv (cost(kv, prompt)) { run E prefill (cost(E, prompt)) growing kv; } } else { } end; }
               ";
    let err = compile_source(&common::main_source(src), &common::horizon(100.0)).unwrap_err();
    assert!(err.contains("`session` belongs inside `workload`"), "{err}");
    refused(
        "branch (o > 2) { hold kv (cost(kv, prompt)) { run E prefill (cost(E, prompt)) growing kv; } } else { }",
        &["hidden from the scheduler"],
    );
}

/// A run whose work reads a hidden attribute reveals it when it ends (the
/// end of a decode is the EOS the scheduler sees): from there the server
/// may decide on it, and on what it set from it.
#[test]
fn a_run_reveals_what_its_work_reads() {
    let after = "set long = o > 2;
                 hold kv (cost(kv, prompt)) { run E prefill (cost(E, prompt)) growing kv; run E decode (cost(E, o - 1)) growing kv; } cache (cost(kv, prompt + o));
                 branch (long) { observe was_long = 1; } else { observe was_long = 0; }
                 branch (o > 3) { observe longer = 1; } else { }";
    compile_source(
        &common::main_source(&program(after)),
        &common::horizon(100.0),
    )
    .unwrap();
    // before the run ends, inside the hold, it is still hidden
    refused(
        "hold kv (cost(kv, prompt)) { run E prefill (cost(E, prompt)) growing kv;
                            branch (o > 2) { run E decode (cost(E, o - 1)) growing kv; } else { run E decode (cost(E, 1)) growing kv; } }",
        &["`o` is hidden from the scheduler, but the server's branch reads it before a run reveals it"],
    );
}

/// Paths join conservatively: revealed on one arm only is not revealed.
#[test]
fn a_reveal_on_one_arm_only_does_not_reveal() {
    refused(
        "hold kv (cost(kv, prompt)) {
           run E prefill (cost(E, prompt)) growing kv;
           branch (prompt > 10) { run E decode (cost(E, o - 1)) growing kv; } else { run E decode (cost(E, 1)) growing kv; }
         }
         branch (o > 2) { observe long = 1; } else { }",
        &["`o` is hidden from the scheduler", "branch"],
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

    for depth in [3, 8] {
        let init = (0..depth)
            .map(|i| format!("set x{i} = 0; "))
            .collect::<String>();
        let mut chain = (0..depth - 1)
            .map(|i| format!("set x{i} = x{}; ", i + 1))
            .collect::<String>();
        chain.push_str(&format!("set x{} = o;", depth - 1));
        let work = "hold kv (cost(kv, prompt)) { run E prefill (cost(E, prompt)) growing kv; }";
        refused(
            &format!("{init} while (x0 == 0) {{ {work} {chain} }}"),
            &["`x0` is set from the hidden `o`", "server's while reads it"],
        );
        refused(
            &format!("{init} loop {{ branch (x0 == 0) {{ observe flag = 1; }} {work} {chain} }}"),
            &[
                "`x0` is set from the hidden `o`",
                "server's branch reads it",
            ],
        );
        // A run revealing o before the decision makes the same chain legal.
        let safe = format!(
            "hold kv (cost(kv, prompt)) {{ run E prefill (cost(E, prompt)) growing kv; run E decode (cost(E, o)) growing kv; }}
          {init} while (x0 == 0) {{ {work} {chain} }}"
        );
        compile_source(
            &common::main_source(&program(&safe)),
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
        branch (flag) { set a = b; }
        run svc (cost(svc, b));
        branch (a > 0) {}
      } }";
    let error = compile_source(&common::main_source(src), &common::horizon(10.0)).unwrap_err();
    // The else path leaves a's original hidden value. Running by b cannot
    // reveal it just because the other path assigned a from b.
    assert!(error.contains("hidden `a`"), "{error}");
    assert!(error.contains("server's branch reads it"), "{error}");
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
        "branch (1) {} else { set a = b; } run svc (cost(svc, a)); branch (b > 0) { observe leaked = b; }",
        "set c = 1 ? a : b; run svc (cost(svc, c)); branch (b > 0) {}",
        "run svc (cost(svc, 1 || b)); branch (b > 0) {}",
    ] {
        let error = compile_source(&model(body), &common::horizon(10.0)).unwrap_err();
        assert!(error.contains("`b` is hidden"), "{error}");
    }
    // Both branch arms depend on b: reading c genuinely reveals b.
    compile_source(
        &model(
            "branch (1) { set c = b; } else { set c = b + 1; }
         run svc (cost(svc, c)); branch (b > 0) {}",
        ),
        &common::horizon(10.0),
    )
    .unwrap();
    // Preserve dependency information after revelation so repeated runs
    // remain evidence at every loop pass, without falsely revealing b.
    compile_source(
        &model(
            "set i = 0; while (i < 2) { run svc (cost(svc, a)); branch (a > 0) {} set i = i + 1; }",
        ),
        &common::horizon(10.0),
    )
    .unwrap();
}

#[test]
fn a_cost_family_annotation_does_not_reveal_its_index() {
    let source = "fn main() { stage svc[2] : delay;
      workload { arrive batch(1); hidden secret; turn { set secret = 0; } }
      server { run svc[0] (cost(svc[secret], 1)); branch (secret > 0) {} }
    }";
    let e = compile_source(source, &common::horizon(10.0)).unwrap_err();
    assert!(e.contains("hidden"), "{e}");
}
