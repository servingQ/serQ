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
    let ok = "hold kv (prompt) { prefill (prompt) growing kv; decode (o - 1) growing kv; } cache (prompt + o);
              observe length = o;";
    compile_source(&common::main_source(&program(ok)), &common::horizon(100.0)).unwrap();
}

#[test]
fn the_server_may_not_decide_on_a_hidden_attribute() {
    refused(
        "branch (o > 2) { hold kv (prompt) { prefill (prompt) growing kv; } } else { observe skipped = 1; }",
        &[
            "`o` is hidden from the scheduler, but the server's branch reads it",
            "decode (o)",
        ],
    );
    refused(
        "choose j in 2 by (o); hold kv (prompt) { prefill (prompt) growing kv; }",
        &["server's choice reads it"],
    );
}

/// What the server sets from a hidden attribute is hidden as well:
/// `Program::validate` sees only `long`, an attribute it may read.
#[test]
fn what_the_server_sets_from_a_hidden_attribute_is_hidden() {
    refused(
        "set long = o > 2; hold kv (long ? prompt : 16) { prefill (prompt) growing kv; }",
        &["`long` is set from the hidden `o` in the server, but the server's admission reads it"],
    );
    refused(
        "set a = o; set b = a + 1; hold kv (prompt) { prefill (prompt) growing kv; grow kv (b); }",
        &["`b` is set from the hidden `o`", "allocation"],
    );
}

/// A `set` late in a loop's body reaches the body's start the next time round.
#[test]
fn a_loop_carries_what_it_sets_round_to_its_start() {
    refused(
        "set n = 0;
         loop { branch (n > 3) { observe long = 1; } else { }
                hold kv (prompt) { prefill (prompt) growing kv; }
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
               session { branch (o > 2) { hold kv (prompt) { prefill (prompt) growing kv; } } else { } end; }
               ";
    let err = compile_source(&common::main_source(src), &common::horizon(100.0)).unwrap_err();
    assert!(err.contains("`session` belongs inside `workload`"), "{err}");
    refused(
        "branch (o > 2) { hold kv (prompt) { prefill (prompt) growing kv; } } else { }",
        &["hidden from the scheduler"],
    );
}

/// A run whose work reads a hidden attribute reveals it when it ends (the
/// end of a decode is the EOS the scheduler sees): from there the server
/// may decide on it, and on what it set from it.
#[test]
fn a_run_reveals_what_its_work_reads() {
    let after = "set long = o > 2;
                 hold kv (prompt) { prefill (prompt) growing kv; decode (o - 1) growing kv; } cache (prompt + o);
                 branch (long) { observe was_long = 1; } else { observe was_long = 0; }
                 branch (o > 3) { observe longer = 1; } else { }";
    compile_source(
        &common::main_source(&program(after)),
        &common::horizon(100.0),
    )
    .unwrap();
    // before the run ends, inside the hold, it is still hidden
    refused(
        "hold kv (prompt) { prefill (prompt) growing kv;
                            branch (o > 2) { decode (o - 1) growing kv; } else { decode (1) growing kv; } }",
        &["`o` is hidden from the scheduler, but the server's branch reads it before a run reveals it"],
    );
}

/// Paths join conservatively: revealed on one arm only is not revealed.
#[test]
fn a_reveal_on_one_arm_only_does_not_reveal() {
    refused(
        "hold kv (prompt) {
           prefill (prompt) growing kv;
           branch (prompt > 10) { decode (o - 1) growing kv; } else { decode (1) growing kv; }
         }
         branch (o > 2) { observe long = 1; } else { }",
        &["`o` is hidden from the scheduler", "branch"],
    );
}
