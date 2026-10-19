//! The two lints, and the corpus they must not fire on.
//!
//! Both come from bugs this repository shipped. Both are errors rather than
//! warnings because neither has a legitimate instance in `examples/` — a
//! warning nobody acts on is worse than no check — so a false positive here
//! is a real cost and `no_false_positives_on_the_corpus` is the test that
//! matters most.

mod common;

use serq::{Overrides, program_path};

/// Compile the deployment `src` with the workload's file `workload`.
fn check(src: &str, workload: &str) -> Result<(), String> {
    common::compile(&common::main_source(src), workload, &common::horizon(500.0)).map(|_| ())
}

/// Engine `llm` admitted `taking` its pools. Five lines, as
/// `a_context_variable_outside_its_moment_is_rejected` reads a line number
/// below them.
fn engine(taking: &str) -> String {
    format!(
        "let bs = 16;
    device gpu {{ kv cap 1e5; }} pool kv on gpu {{ block bs; evict lru; }}
    engine llm on gpu {{ reqs cap 8; tokens cap 512; scheduler {{ advance running; admit waiting while (running.preempted == 0) taking {taking}; }} worker {{ forward (1e-3); }} }}
    pool reqs on llm {{ }}

    "
    )
}
const ENGINE_WORKLOAD: &str = "arrive poisson(0.3); init { set K = 0; }
               turn { set n = ceil(~exp(500)); set o = floor(~exp(200)) + 1; }";
/// The workload of `engine`'s programs, sending `send` and running `session`.
fn engine_workload(send: &str, session: &str) -> String {
    format!("workload {{ {ENGINE_WORKLOAD} send ({send}); session {{ {session} }} }}")
}
/// The stale-header programs' workload: the context `K` grows each turn.
fn multi_turn() -> String {
    engine_workload("K, n, o", "loop { turn; set K = K + n + o; end; }")
}
/// The admission of the stale-header programs below.
const CACHED_PREFIX: &str = "reqs (1), kv (c + min(request.input - c, budget_left(llm))) cache (request.input + request.output)";

/// `examples/multi-turn/vllm.sq` shipped with a prefix-cache lookup bound before the
/// session queued, under a comment citing the admission-time lookup. The
/// engine's admission is read when the scheduler admits the request; a `set`
/// above the call is not.
#[test]
fn a_stale_header_read_is_rejected() {
    let llm = engine(CACHED_PREFIX);
    let src = format!(
        "{llm}
        server (K, n, o) {{
          set prompt = K + n;
          set c = min(cachedin(kv), prompt - 1);
          llm (prompt, o);
        }}"
    );
    let e = check(&src, &multi_turn()).expect_err("rejected");
    assert!(e.contains("`c` reads cachedin(kv)"), "{e}");
}

/// `request.hit`, read at the admission, is the way to say it.
#[test]
fn the_request_s_hit_is_the_way_through() {
    let llm = engine(
        "reqs (1), kv (request.hit + min(request.input - request.hit, budget_left(llm))) \
         reuse (request.input - 1) cache (request.input + request.output)",
    );
    let src = format!(
        "{llm}
        server (K, n, o) {{
          set prompt = K + n;
          llm (prompt, o);
        }}"
    );
    check(&src, &multi_turn()).expect("the request's hit is the way to say it");
}

/// Reassigning before the call means the stale value never reaches the
/// admission.
#[test]
fn a_reassignment_clears_the_lint() {
    let llm = engine("reqs (1), kv (c + request.input) cache (request.input)");
    let src = format!(
        "{llm}
        server (K, n, o) {{
          set prompt = K + n;
          set c = min(cachedin(kv), prompt - 1);
          set c = 0;
          llm (prompt, 1);
        }}"
    );
    check(&src, &multi_turn()).expect("the read no longer reaches the header");
}

/// A read after the request is not a header's.
#[test]
fn a_read_after_the_request_is_fine() {
    let llm = engine("reqs (1), kv (request.input) cache (request.input)");
    let src = format!(
        "{llm}
        server (K, n, o) {{
          set prompt = K + n;
          llm (prompt, 1);
          set c = min(cachedin(kv), prompt - 1);
          observe hit = c;
        }}"
    );
    check(&src, &multi_turn()).expect("after admission is not stale");
}

/// A constant guard strictly between 0 and 1 is not a test; it was meant as
/// a draw, and the link error names the spelling for that.
#[test]
fn a_constant_probability_guard_is_rejected() {
    let src = "stage tool : delay;
        server (Z) { run tool (cost(tool, Z));
        }
        ";
    let wl = "workload { arrive poisson(1); turn { set Z = ~exp(3); } send (Z);
          session {  loop { if (0.8) { turn;  } else { end; } }
          }
        }
        ";
    let e = check(src, wl).expect_err("rejected");
    assert!(e.contains("`if (0.8)` is not a test"), "{e}");
    assert!(e.contains("if (~bernoulli(0.8))"), "{e}");
    // a constant that is neither 0 nor 1 and not a probability either
    let e = check(src, &wl.replace("if (0.8)", "if (2)")).expect_err("rejected");
    assert!(
        e.contains("`if (2)` is not a test: a guard is 0 or 1."),
        "{e}"
    );
    assert!(!e.contains("bernoulli"), "{e}");
    check(src, &wl.replace("if (0.8)", "if (1)")).expect("0 and 1 are tests");
}

/// 0 and 1 are tests, not draws, and stay legal.
#[test]
fn zero_and_one_are_tests() {
    for g in ["0", "1"] {
        let src = "stage tool : delay;
        server (Z) { run tool (cost(tool, Z));
        }
        ";
        let wl = format!(
            "workload {{ arrive poisson(1); turn {{ set Z = ~exp(3); }} send (Z);
          session {{  loop {{ if ({g}) {{ turn;  }} else {{ end; }} }}
          }}
        }}
        "
        );
        check(src, &wl).unwrap_or_else(|e| panic!("`if ({g})` is a test: {e}"));
    }
}

/// A context variable outside the moment that supplies it. `tokens` is what a
/// step stage's budget and cost see for one iteration; in a session
/// statement it used to read as 0 and the program ran. The IR's validator
/// knows the table, and the linker now runs it on what it produces.
#[test]
fn a_context_variable_outside_its_moment_is_rejected() {
    let llm = engine("reqs (1), kv (request.input)");
    let src = format!(
        "{llm}
    server (n) {{ set x = tokens;
          llm (n, 1);
        }}"
    );
    let e = check(&src, &engine_workload("n", "turn; end;")).expect_err("rejected");
    // the block's role, after the statement's place in the text (#279)
    assert!(
        e.starts_with("deployment.sq: 7:26: session: "),
        "names the line and the block's role: {e}"
    );
    assert!(e.contains("set x = tokens;\n"), "shows the line: {e}");
    assert!(e.contains("`tokens` is read in a session statement"), "{e}");
    assert!(e.contains("exists only in an engine's `worker`"), "{e}");
    // every other moment refuses what it does not supply, and says where it
    // was read and where it exists
    const LLM: &str = "reqs cap 8; tokens cap 512; scheduler { advance running; admit waiting while (running.preempted == 0) taking reqs (1), kv (request.input); } worker { forward (1e-3); }";
    let engine = |pool: &str, llm: &str, session: &str| {
        format!(
            "let bs = 16;
        device gpu {{ kv cap 1e5; }}
        engine llm on gpu {{ {llm} }}
        pool kv on gpu {{ block bs; {pool} }}
        pool reqs on llm {{ }}
        stage svc : ps (4);

        server (m) {{ {session} llm (m, 1);
        }}"
        )
    };
    let wl = "workload { arrive poisson(0.3); init { set K = 0; }
          turn { set m = ceil(~exp(500)); set o = floor(~exp(200)) + 1; }
          send (m);
          session {  turn; end;
          }
        }";
    // (the prompt is `m`: a session attribute named `present` would shadow
    // the ps capacity variable)
    for (src, where_read, exists) in [
        (
            engine("evict by (tokens);", LLM, ""),
            "pool `kv`: `tokens` is read in an eviction key",
            "an engine's `worker`",
        ),
        (
            engine("queue by (age);", LLM, ""),
            "pool `kv`: `age` is read in a pool's queue keys, read before selecting a waiting session",
            "an eviction key",
        ),
        (
            engine(
                "",
                LLM,
                "hold kv (cost(kv, size)) { run svc (cost(svc, 1)); }",
            ),
            "session: `size` is read in a hold's header, read at admission",
            "an eviction key",
        ),
        (
            engine(
                "",
                LLM,
                "hold kv (cost(kv, 1)) { run svc (cost(svc, present)); }",
            ),
            "session: `present` is read in a session statement",
            "a ps stage's capacity",
        ),
        // an engine names the batch `batch.…`, which its parser refuses
        // before the batch is formed; a variable of another moment is the
        // linker's
        (
            engine(
                "",
                &LLM.replace("tokens cap 512", "tokens cap age + 512"),
                "",
            ),
            "engine `llm`: `age` is read in an engine's `tokens cap` or `each at most`",
            "an eviction key",
        ),
        (
            engine(
                "",
                &LLM.replace("tokens cap 512", "tokens cap batch.tokens + 512"),
                "",
            ),
            "`tokens cap` is read before the batch is formed",
            "`batch.tokens` is read in `worker`",
        ),
        (
            engine(
                "",
                &LLM.replace(
                    "advance running;",
                    "advance running each at most (batch.attention);",
                ),
                "",
            ),
            "`each at most` is read before the batch is formed",
            "`batch.attention` is read in `worker`",
        ),
        (
            engine("", &LLM.replace("forward (1e-3)", "forward (age)"), ""),
            "engine `llm`: `age` is read in an engine's `worker`",
            "an eviction key",
        ),
        (
            engine("", LLM, "set x = init_age; set y = age;"),
            "init: `init_age`",
            "",
        ),
    ] {
        if where_read.starts_with("init:") {
            // an `init` statement is named as such
            let wl = wl.replace("init { set K = 0; }", "init { set K = age; }");
            let e = check(&engine("", LLM, ""), &wl).expect_err("rejected");
            assert!(e.contains(": init: `age`"), "{e}");
            continue;
        }
        let e = match check(&src, wl) {
            Err(e) => e,
            Ok(()) => panic!("linked, but should have said `{where_read}`"),
        };
        assert!(e.contains(where_read), "want `{where_read}` in: {e}");
        assert!(e.contains(exists), "want `{exists}` in: {e}");
    }
    // what the table allows still links: `age`, `size` and a pool index in
    // an eviction key, `present` in a ps capacity, the residents' variables in a
    // `tokens cap` and an `each at most`, the batch's in `worker`, `now` at
    // every moment (under `floor` in a `tokens cap`, a number of tokens, #661)
    let src =
        "device gpu[2] { kv cap 1e5; }
        pool kv on gpu { evict by (age, size + used(kv[size > 1e9 ? 1 : 0]) * 0, now * 0); }
        stage svc : ps (min(present, 4) + now * 0);
        device chip { }
        engine llm on chip {
          tokens cap 512 + running.count + running.decoding + running.kv_decode * 0 + running.kv_prefill * 0 + floor(now * 0);
          scheduler {
            let c = running.decoding > 0 ? 64 : 128;
            advance running each at most (c);
            admit waiting while (running.preempted == 0) each at most (c);
          }
          worker { forward (1e-3 * batch.tokens + batch.prefilled * 0 + batch.attention * 0 + now * 0); }
        }
        server (n) { set t = now; hold kv[0] (cost(kv, n)) { run svc (cost(svc, n)); } llm (1, 1);
        }
        ";
    let wl = "workload { arrive poisson(0.3); turn { set n = ~exp(500); } send (n);
          session {  turn; end;
          }
        }";
    check(src, wl).expect("links");
}

/// A program whose engine `llm` serves its residents by `advance`, a
/// schedule statement, and then admits as vLLM does.
fn advancing(advance: &str) -> String {
    format!(
        "device gpu {{ kv cap 1e5; }}
        engine llm on gpu {{
          tokens cap 512;
          scheduler {{ {advance} admit waiting while (running.preempted == 0) taking kv (1); }}
          worker {{ forward (1); }}
        }}
        pool kv on gpu {{ }}
        server {{ llm (1, 1);
        }}
        "
    )
}

/// The workload of `advancing`'s programs: one session of one turn.
const ONE_TURN: &str = "workload { arrive batch(1); session { turn; end; } }";

/// An engine whose schedule advances its residents says their order once, on
/// `advance running`, in one of three spellings or as `exclusive prefill`.
#[test]
fn the_residents_order_is_said_on_advance_running() {
    for ok in [
        "advance running;",
        "advance running admission;",
        "advance running decode first;",
        "exclusive prefill;",
    ] {
        check(&advancing(ok), ONE_TURN).unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
    let e = check(&advancing("advance running shortest;"), ONE_TURN).expect_err("unknown");
    assert!(e.contains("found `shortest`"), "{e}");
}

/// `advance running admission` is `by` with no keys (every resident ties, and
/// ties are admission order), so the IR knows one form; a serve key may read
/// the residents' variables and may not draw.
#[test]
fn serve_admission_is_by_with_no_keys_and_a_key_does_not_draw() {
    let ir = |s: &str| {
        common::compile(
            &common::main_source(&advancing(s)),
            ONE_TURN,
            &common::horizon(10.0),
        )
        .unwrap()
        .to_json()
    };
    assert_eq!(ir("advance running admission;"), ir("advance running;"));
    assert!(ir("advance running admission;").contains("\"By\": []"));
    check(
        &advancing("advance running by (running.count > 4 ? -remaining : admission);"),
        ONE_TURN,
    )
    .expect("the residents' variables are keys");
    let e =
        check(&advancing("advance running by (~uniform(0, 1));"), ONE_TURN).expect_err("a draw");
    assert!(e.contains("engine `llm`"), "{e}");
    assert!(
        e.contains("an `advance running by (…)` key may not draw"),
        "{e}"
    );
}

/// `hidden o;`: the scheduler does not know the output length (vLLM knows
/// `max_tokens`, scheduler.py:639, and learns the length when `check_stop`
/// sees EOS or the cap, sched/utils.py:98-119). A hidden attribute is read
/// in session statements and rejected at every scheduler moment.
#[test]
fn a_hidden_attribute_is_not_read_by_the_scheduler() {
    let wl = |taking: &str| {
        format!(
            "let bs = 16;
        device gpu {{ kv cap 1e5; }}
        engine llm on gpu {{
          reqs cap 8;
          tokens cap 512;
          scheduler {{ advance running; admit waiting while (running.preempted == 0) taking {taking}; }}
          worker {{ forward (1e-3); }}
        }}
        pool kv on gpu {{ block bs; evict lru; }}
        pool reqs on llm {{ }}

        "
        )
    };
    let wl_workload = "workload { arrive poisson(0.3); init { set K = 0; }
                   turn { set n = ceil(~exp(500)); set o = floor(~exp(200)) + 1; } send (n, o);
                   session {  turn; end; \n} }";
    // the request's output may be it, and its release may cache by it
    let ok = format!(
        "{}\nserver (n, hidden o) {{ llm (n, o);\n}}",
        wl("reqs (1), kv (request.input) cache (request.input + request.output)")
    );
    check(&ok, wl_workload).expect("links");
    // the admission may not: the reservation is the scheduler's
    let bad = format!(
        "{}\nserver (n, hidden o) {{ llm (n, o);\n}}",
        wl("reqs (1), kv (request.input) reserve (n + o)")
    );
    let e = check(&bad, wl_workload).expect_err("rejected");
    assert!(e.contains("`o` is hidden from the scheduler"), "{e}");
    assert!(e.contains("admits the request"), "{e}");
    // nor through the request's output, which the call passes it as
    let bad = format!(
        "{}\nserver (n, hidden o) {{ llm (n, o);\n}}",
        wl("reqs (1), kv (request.input) reserve (request.input + request.output)")
    );
    let e = check(&bad, wl_workload).expect_err("rejected");
    assert!(e.contains("`o` is hidden from the scheduler"), "{e}");
    // nor a pool's queue key
    let bad = "let bs = 16;
        device gpu { kv cap 1e5; }
        engine llm on gpu {
          tokens cap 512;
          scheduler { advance running; admit waiting while (running.preempted == 0) taking kv (request.input); }
          worker { forward (1e-3); }
        }
        pool kv on gpu { block bs; evict lru; queue by (o); }

        server (n, hidden o) { llm (n, 1);
        }";
    let e = check(bad, wl_workload).expect_err("rejected");
    assert!(e.contains("pool `kv`"), "{e}");
    assert!(e.contains("`o` is hidden from the scheduler"), "{e}");
    // nor an engine's serve key, read for each running request
    let bad = "let bs = 16;
        device gpu { kv cap 1e5; }
        engine llm on gpu {
          tokens cap 512;
          scheduler { advance running by (o); admit waiting while (running.preempted == 0) taking kv (request.input); }
          worker { forward (1e-3); }
        }
        pool kv on gpu { block bs; evict lru; }

        server (n, hidden o) { llm (n, 1);
        }";
    let e = check(bad, wl_workload).expect_err("rejected");
    assert!(e.contains("engine `llm`"), "{e}");
    assert!(e.contains("`o` is hidden from the scheduler"), "{e}");
    // a name nothing sets is not an attribute
    let bad = "pool kv { cap 1e5; }
        stage svc : fifo;

        server (hidden nothing) { hold kv (cost(kv, 1)) { run svc (cost(svc, 1)); }
        }";
    let nothing = "workload { arrive poisson(0.3); send (nothing);
          session { turn; end;
          }
        }";
    let e = check(bad, nothing).expect_err("rejected");
    assert!(
        e.contains("`send (nothing)` sends the workload's `nothing`, which nothing sets"),
        "{e}"
    );
    // what the scheduler sets cannot be hidden from it, and a name is hidden once
    let one_pool = format!(
        "{}\nserver (n, hidden o) {{ llm (n, 1);\n}}",
        wl("kv (request.input)")
    );
    let bad = one_pool.replace("hidden o)", "hidden computed)");
    let computed = wl_workload.replace("send (n, o);", "send (n, computed: 0);");
    let e = check(&bad, &computed).expect_err("rejected");
    assert!(
        e.contains("hidden `computed`: the scheduler sets it"),
        "{e}"
    );
    let bad = one_pool.replace("hidden o)", "hidden o, hidden o)");
    let e = check(&bad, wl_workload).expect_err("rejected");
    assert!(e.contains("duplicate parameter `o`"), "{e}");
}

/// A stage's capacity, budget and cost, and an iteration body's guard, are
/// read for the stage, where no one session is. `ps(1 + prompt)` linked and
/// read the attribute as NaN: the run ended no session and said nothing.
#[test]
fn a_stage_reads_no_session_attribute() {
    let ps = "stage svc : ps(1 + prompt);
        server (prompt) { run svc (cost(svc, 1)); }";
    let wl = "workload { arrive poisson(0.5); init { set prompt = 2; } send (prompt); }";
    let e = check(ps, wl).expect_err("rejected");
    assert!(e.contains("`prompt` is a session attribute"), "{e}");
    assert!(e.contains("a ps stage's capacity"), "{e}");
    let engine = |tokens: &str, schedule: &str, execute: &str| {
        format!(
            "device gpu {{ kv cap 100; }}
            engine llm on gpu {{
              tokens cap {tokens};
              scheduler {{ {schedule} }}
              worker {{ forward ({execute}); }}
            }}
            pool kv on gpu {{ }}
            server (prompt) {{ llm (prompt, 1); }}"
        )
    };
    let vllm = "advance running; admit waiting while (running.preempted == 0) taking kv (prompt);";
    check(&engine("8", vllm, "1"), wl).expect("links");
    for (src, moment) in [
        (engine("8 + prompt", vllm, "1"), "an engine's `tokens cap`"),
        (engine("8", vllm, "1 + prompt"), "an engine's `worker`"),
        (
            engine(
                "8",
                "if (prompt > 0) { advance running; } \
                 admit waiting while (running.preempted == 0) taking kv (prompt);",
                "1",
            ),
            "an engine's `scheduler`",
        ),
    ] {
        let e = check(&src, wl).expect_err("rejected");
        assert!(e.contains("`prompt` is a session attribute"), "{e}");
        assert!(e.contains(moment), "{e}");
    }
}

/// The lints exist to be errors, which is only defensible if nothing real
/// trips them.
#[test]
fn no_false_positives_on_the_corpus() {
    for name in [
        "mg1",
        "ps",
        "closed",
        "replica",
        "routing",
        "vllm",
        "vllm_request",
        "vllm_replay",
    ] {
        let path = program_path(name);
        let workload = serq::paired_workload(&path).expect("a pair");
        serq::load_paired(&path, &workload, &common::horizon(500.0))
            .unwrap_or_else(|e| panic!("{name} is a real program and must link: {e}"));
    }
}

/// A context variable renamed for what it means (#139) says its new name,
/// and in an engine the engine's word for it (#428).
#[test]
fn an_old_context_variable_name_says_the_new_one() {
    // in an engine the old name is answered with the engine's word (#428);
    // elsewhere with the context variable's new name
    for (old, says) in [
        ("ntok", "in an engine, `ntok` is `batch.tokens` here"),
        ("kvb", "in an engine, `kvb` is `batch.kv_decode` here"),
        ("queued", "`queued` is now `waiting`"),
    ] {
        let (key, cost) = if old == "queued" {
            (old, "1")
        } else {
            ("size", old)
        };
        let src = format!(
            "device gpu {{ kv cap 10; }}\nengine e on gpu {{ tokens cap inf; scheduler {{ advance running; admit waiting while (running.preempted == 0); }} worker {{ forward ({cost}); }} }}\npool kv on gpu {{ evict by ({key}); }}\nserver {{\n}}\n"
        );
        let e = check(&src, "workload { session { turn; end; \n} }").unwrap_err();
        assert!(e.contains(says), "{e}");
    }
}

/// Engine `llm` admitted `taking` its pools, and the server that calls it
/// after a `user` delay, every turn.
fn cache_engine(taking: &str) -> String {
    cache_server(&format!(" taking {taking}"), "", "llm (1000, 1);")
}

/// Engine `llm` with `admit waiting … {taking}`, pool `pool` declared after
/// it, and a server that runs `user`, then `server`, every turn.
fn cache_server(taking: &str, pool: &str, server: &str) -> String {
    format!(
        "device gpu {{ kv cap 1e5; }}
    engine llm on gpu {{
      tokens cap 512;
      scheduler {{ advance running; admit waiting while (running.preempted == 0){taking}; }}
      worker {{ forward (1e-3); }}
    }}
    pool kv on gpu {{ block 16; evict lru; }}
    stage user : delay;
    {pool}
        server {{ loop {{ run user (cost(user, 1));
            {server}
          }}
        }} "
    )
}
/// `cache_server`'s workload: one closed client, a turn each time round.
const CACHE_ENGINE_WORKLOAD: &str = "workload { arrive closed(1); session { turn;
        } }";

/// The server's hold of the device's pool, which no engine admits, around
/// `body`, with `clause` after it; then a request of the engine, which a
/// stage no statement runs would not link without.
fn holding(body: &str, clause: &str) -> String {
    cache_server(
        "",
        "",
        &format!("hold kv (cost(kv, 1000)) {{ {body} }}{clause} llm (1, 1);"),
    )
}

/// #230: a hold without a `cache` clause consumes nothing of the session's
/// prefix, so `cached` is 0 in its body; reading it there is the old idiom
/// for a probe that consumed and kept nothing, which is now `cache (0)`.
#[test]
fn cached_in_a_hold_without_cache_is_rejected() {
    let src = holding("observe hit = cached > 0; run user (cost(user, 1));", "");
    let e = check(&src, CACHE_ENGINE_WORKLOAD).expect_err("rejected");
    assert!(
        e.contains("`cached` is read in a hold on `kv` that has no `cache` clause"),
        "{e}"
    );
    assert!(e.contains("`cache (0)`"), "names the spelling for it: {e}");
}

/// `cache (0)` is that spelling, and the usual request links as before.
#[test]
fn cache_zero_is_the_way_through() {
    for clause in [" cache (cost(kv, 0));", " cache (cost(kv, 1000));"] {
        let src = holding(
            "observe hit = cached > 0; run user (cost(user, 1));",
            clause,
        );
        check(&src, CACHE_ENGINE_WORKLOAD).unwrap_or_else(|e| panic!("{clause}: {e}"));
    }
}

/// The innermost hold is the admission that set `cached`: a hold on
/// another pool nested in the request's reads its own, which is 0.
///
/// A request holds what its `taking` takes, once, so the two holds are the
/// server's, of pools no engine admits, and the engine's request runs in
/// the inner one; its argument is substituted where the prefill reads it.
#[test]
fn cached_in_a_nested_hold_on_another_pool_is_rejected() {
    let nested = |inner: &str| {
        cache_server(
            "",
            "pool reqs { cap 4; }",
            &format!("hold kv (cost(kv, 1000)) {{ {inner} }} cache (cost(kv, 1000));"),
        )
    };
    let src = nested("hold reqs (cost(reqs, 1)) { llm (1000 - cached, 1); }");
    let e = check(&src, CACHE_ENGINE_WORKLOAD).expect_err("rejected");
    assert!(e.contains("hold on `reqs`"), "{e}");
    let src = nested("set c = cached; hold reqs (cost(reqs, 1)) { llm (1000 - c, 1); }");
    check(&src, CACHE_ENGINE_WORKLOAD)
        .expect("read above the inner hold, as llmd_nixl_pull.sq does");
    // read after the inner hold, in the outer body: the inner admission set
    // it, so `c` was 0 every turn where 992 was kept (#237); in a branch
    // after it, and in a loop whose next iteration comes after it
    let inner = "hold reqs (cost(reqs, 1)) { run user (cost(user, 0.01)); }";
    for body in [
        format!("{inner} observe c = cached;"),
        format!("{inner} if (1 > 0) {{ observe c = cached; }}"),
        format!("loop {{ observe c = cached; {inner} }}"),
    ] {
        let e = check(&nested(&body), CACHE_ENGINE_WORKLOAD).expect_err("rejected");
        assert!(
            e.contains("`cached` is read after the hold on `reqs` in the body of the hold on `kv`"),
            "{body}\n{e}"
        );
    }
}

/// `reuse` bounds what the admission consumes; without `cache` it consumes nothing.
#[test]
fn reuse_without_cache_is_rejected() {
    let src = cache_server(
        "",
        "",
        "hold kv (cost(kv, 1000)) reuse (cost(kv, 512)) { run user (cost(user, 1)); }",
    );
    let e = check(&src, CACHE_ENGINE_WORKLOAD).expect_err("rejected");
    assert!(
        e.contains("`reuse` on a hold on `kv` that has no `cache` clause"),
        "{e}"
    );
    // an engine's admission likewise
    let e = check(
        &cache_engine("kv (1000) reuse (512)"),
        CACHE_ENGINE_WORKLOAD,
    )
    .expect_err("rejected");
    assert!(e.contains("`reuse`"), "{e}");
    check(
        &cache_engine("kv (1000) reuse (512) cache (1000)"),
        CACHE_ENGINE_WORKLOAD,
    )
    .expect("links");
}

/// Two closed clients, a turn each time round: the workload of the name
/// tests below.
const CLOSED: &str = "workload { arrive closed(2); session { turn; } }";

/// #231: a name resolves to an attribute first, then a `let`, then a context
/// variable, so `set present = 500;` anywhere made `ps(min(present, 16))`
/// read the attribute: capacity 16 for 2 jobs, a service time of 0.125
/// where the jobs present give 1. Neither an attribute nor a `let` may take
/// a name the language supplies (a context variable, `inf`).
#[test]
fn a_context_variable_name_cannot_be_an_attribute_or_a_constant() {
    const PS: &str = "stage dec : ps(min(present, 16));
        stage user : delay;
        server { loop { SET run dec (cost(dec, 1)); observe r = now; run user (cost(user, 1)); }
        }
        ";
    let e = check(&PS.replace("SET", "set present = 500;"), CLOSED).expect_err("rejected");
    assert!(
        e.contains("`present` is a name the language supplies, read in a ps stage's capacity"),
        "{e}"
    );
    assert!(e.contains("a session attribute named `present`"), "{e}");
    let e = check(
        &format!("let present = 500; {}", PS.replace("SET", "")),
        CLOSED,
    )
    .expect_err("rejected");
    assert!(e.contains("a `let` constant named `present`"), "{e}");
    // `inf` is folded at parse time, so the answer would be right, but the
    // name is the language's, as it is for an aggregate's index.
    let e = check(&PS.replace("SET", "set inf = 500;"), CLOSED).expect_err("rejected");
    assert!(
        e.contains("`inf` is a name the language supplies, read in every expression"),
        "{e}"
    );
    // Renamed, the program links, and the capacity reads the jobs present:
    // `ps(φ)` serves each job at `φ(present)/present` = 1 while at most 16
    // are present, so a unit of work takes exactly 1 s whatever the arrivals
    // (the attribute's 0.125 needs the two jobs together: 16/2 = 8).
    let src = PS.replace("SET", "set prompt = 500;");
    check(&src, CLOSED).expect("links");
    let r = common::run(
        &common::main_source(&src),
        CLOSED,
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(50.0)
        },
    )
    .unwrap();
    assert_eq!(r.stage("dec").unwrap().mean_service, 1.0, "{}", r.text());
}

/// #460: the same holds for a declaration and a parameter. `def now(x)`
/// beside `now`, or `observe admitted` beside `admitted`, linked, and one
/// word meant the program's thing in one place and the language's in
/// another. An engine's list (`running.count`) is the language's as well.
#[test]
fn a_declaration_cannot_take_a_name_the_language_supplies() {
    const SRC: &str = "DECL
        stage svc : fifo;
        server { run svc (cost(svc, 1)); observe response = now; }
        ";
    for (decl, refused) in [
        ("def now(x) { x * 2 }", "a definition named `now`"),
        ("def running(x) { x * 2 }", "a definition named `running`"),
        ("def f(now) { now * 2 }", "a parameter named `now`"),
        ("gauge present = 1;", "a gauge named `present`"),
        ("stage size : delay;", "a stage named `size`"),
        ("pool position { cap 1; }", "a pool named `position`"),
        ("claim tokens : at end (1);", "a claim named `tokens`"),
        ("device now { kv cap 1; }", "a device named `now`"),
        (
            "device gpu { step_time (age) = 1; }",
            "a parameter named `age`",
        ),
        (
            "device gpu { kv cap 1; } engine served on gpu { worker { forward (1); } }",
            "an engine named `served`",
        ),
        // the engine's lists are the language's for #231's check too
        ("let running = 1;", "a `let` constant named `running`"),
    ] {
        let e = check(&SRC.replace("DECL", decl), CLOSED).expect_err(decl);
        assert!(
            e.contains(&format!(
                "{refused} would make the one word mean two things"
            )),
            "{decl}: {e}"
        );
    }
    let set = CLOSED.replace("session { turn; }", "session { set running = 1; turn; }");
    let e = check(&SRC.replace("DECL", ""), &set).expect_err("rejected");
    assert!(
        e.contains("a session attribute named `running` would make the one word mean two things"),
        "{e}"
    );
    let observe = SRC.replace("DECL", "").replace(
        "observe response",
        "observe admitted = now; observe response",
    );
    let e = check(&observe, CLOSED).expect_err("rejected");
    assert!(
        e.contains("`admitted` is a name the language supplies, read in an engine's `scheduler`"),
        "{e}"
    );
    // renamed, it links
    check(
        &observe.replace("observe admitted", "observe admitted_at"),
        CLOSED,
    )
    .expect("links");
}

/// A schedule's `let` is a declaration too: `let inf = 1;` in vllm.sq's
/// schedule linked, and its threshold's `: inf` read 1.
#[test]
fn a_schedules_let_cannot_take_a_name_the_language_supplies() {
    let src = format!(
        "{}
        server (n) {{ llm (n, 1); }}",
        engine("reqs (1)").replace("scheduler { ", "scheduler { let inf = 1; ")
    );
    let wl = engine_workload("n", "turn;");
    let e = check(&src, &wl).expect_err("rejected");
    assert!(
        e.contains(
            "a `let` of engine `llm`'s schedule named `inf` would make the one word mean two things"
        ),
        "{e}"
    );
    check(&src.replace("let inf = 1; ", ""), &wl).expect("links");
}

/// #470: nor may a declaration share its name with an attribute, built in
/// or set anywhere in the program. `stage think` beside the attribute
/// `think`, or `set svc` beside `stage svc`, linked, and a bare name read
/// the one or the other by its place alone (#467). Writing a built-in
/// attribute is a `set`, and stays legal.
#[test]
fn a_declaration_cannot_share_its_name_with_an_attribute() {
    const SRC: &str = "DECL
        stage svc : fifo;
        server { run svc (cost(svc, 1)); observe response = now; }
        ";
    let refusal = |src: &str, wl: &str| check(src, wl).expect_err("refused");
    let two_things = "would make the one word mean two things";
    for (decl, refused) in [
        (
            "stage think : delay;",
            "`think` is a built-in attribute; a stage named `think`",
        ),
        (
            "def more(x) { x }",
            "`more` is a built-in attribute; a definition named `more`",
        ),
        (
            "gauge serial = 1;",
            "`serial` is a built-in attribute; a gauge named `serial`",
        ),
    ] {
        let e = refusal(&SRC.replace("DECL", decl), CLOSED);
        assert!(
            e.contains(&format!("{refused} {two_things}")),
            "{decl}: {e}"
        );
    }
    let src = SRC.replace("DECL", "");
    let observe = |name: &str| {
        src.replace(
            "observe response",
            &format!("observe {name} = 1; observe response"),
        )
    };
    let set = |wl: &str, name: &str| {
        wl.replace(
            "session { turn; }",
            &format!("session {{ set {name} = 1; turn; }}"),
        )
    };
    let e = refusal(&observe("out"), CLOSED);
    assert!(
        e.contains(&format!("an observation named `out` {two_things}")),
        "{e}"
    );
    // the `set` comes after the declaration, or before it
    let e = refusal(&src, &set(CLOSED, "svc"));
    assert!(
        e.contains(&format!(
            "`svc` is an attribute the program sets at workload.sq:1:44; a stage named `svc` {two_things}"
        )),
        "{e}"
    );
    let e = refusal(&observe("hit"), &set(CLOSED, "hit"));
    assert!(
        e.contains(&format!("an observation named `hit` {two_things}")),
        "{e}"
    );
    assert!(
        e.contains("help: rename the declaration or the attribute"),
        "{e}"
    );
    check(&observe("hit"), &set(&set(CLOSED, "warm"), "more")).expect("links");
}

/// #486: nor may two declarations of different kinds share a name.
/// `mg1.sq` declared `def service()`, the law, and `observe service = s`, a
/// sample of it. Two of one kind are each kind's own check: `observe x`
/// again is the one statistic recorded at another place. A queue's own pool
/// is `Q.kv`, read bare only in the queue's body, which reads a `def` but
/// no other top-level declaration. The refusal names both places, whatever
/// the order (`engine Q` above or below `queue Q`).
#[test]
fn a_declaration_cannot_share_its_name_with_another() {
    const SRC: &str = "DECL
        stage svc : fifo;
        server { run svc (cost(svc, 1)); OBSERVE observe response = now; }
        ";
    const ENGINE: &str = "device gpu { } engine Q on gpu { tokens cap 8; scheduler { advance running; admit waiting while (running.preempted == 0); } worker { forward (1); } }";
    const QUEUE: &str = "queue Q { pool kv { cap 1; } serve fifo; }";
    let schedule_let =
        |l: &str| ENGINE.replace("scheduler { ", &format!("scheduler {{ let {l} = 1; "));
    let two_things = "would make the one word mean two things";
    let rename = "help: rename one of the two declarations";
    let refused = |decl: &str, observe: &str| {
        check(
            &SRC.replace("DECL", decl).replace("OBSERVE", observe),
            CLOSED,
        )
        .expect_err(decl)
    };
    for (decl, observe, want) in [
        (
            "def service() { 1 }".to_string(),
            "observe service = 1;",
            "`service` is a definition at 1:17; an observation named `service`",
        ),
        (
            String::new(),
            "observe svc = 1;",
            "`svc` is a stage at 2:15; an observation named `svc`",
        ),
        (
            "gauge g = 1; claim g : at end (1 > 0);".to_string(),
            "",
            "`g` is a gauge at 1:19; a claim named `g`",
        ),
        (
            format!("def kv() {{ 1 }} {QUEUE}"),
            "",
            "`kv` is a definition at 1:17; a pool of queue `Q` named `kv`",
        ),
        (
            "queue D[2] { pool kv { cap 1; } serve fifo; } def kv() { 1 }".to_string(),
            "",
            "`kv` is a pool of queue `D` at 1:31; a definition named `kv`",
        ),
        (
            format!("{ENGINE} {QUEUE}"),
            "",
            "`Q` is an engine at 1:35; a queue named `Q`",
        ),
        (
            format!("{QUEUE} {ENGINE}"),
            "",
            "`Q` is a queue at 1:19; an engine named `Q`",
        ),
        // a `let` is read everywhere, a queue's body too (#488)
        (
            "let svc = 1;".to_string(),
            "",
            "`svc` is a `let` constant at 1:17; a stage named `svc`",
        ),
        (
            "let p = 1; pool p { cap 1; }".to_string(),
            "",
            "`p` is a `let` constant at 1:17; a pool named `p`",
        ),
        (
            "def f() { 1 } let f = 1;".to_string(),
            "",
            "`f` is a definition at 1:17; a `let` constant named `f`",
        ),
        (
            "let f = 1; def f() { 1 }".to_string(),
            "",
            "`f` is a `let` constant at 1:17; a definition named `f`",
        ),
        (
            format!("let kv = 1; {QUEUE}"),
            "",
            "`kv` is a `let` constant at 1:17; a pool of queue `Q` named `kv`",
        ),
        (
            format!("{QUEUE} let kv = 1;"),
            "",
            "`kv` is a pool of queue `Q` at 1:28; a `let` constant named `kv`",
        ),
        // a schedule's `let` is read in its `each at most`, as a `def` is
        (
            format!("def c() {{ 1 }} {}", schedule_let("c")),
            "",
            "`c` is a definition at 1:17; a `let` of engine `Q`'s schedule named `c`",
        ),
        // and a program's `let`, which its own would shadow there
        (
            format!("let k = 2; {}", schedule_let("k")),
            "",
            "`k` is a `let` constant at 1:17; a `let` of engine `Q`'s schedule named `k`",
        ),
    ] {
        let e = refused(&decl, observe);
        assert!(e.contains(&format!("{want} {two_things}")), "{decl}: {e}");
        assert!(e.contains(rename), "{e}");
    }
    // a pool and a stage are refused for the reason `cost(NAME, …)` gives
    let e = refused("pool p { cap 1; } stage p : fifo;", "");
    assert!(
        e.contains(&format!(
            "`p` is a pool at 1:18; a stage named `p` {two_things}\n"
        )),
        "{e}"
    );
    assert!(
        e.contains(&format!(
            "{rename}: a pool and a stage share one name space, as `cost(p, …)` reads either"
        )),
        "{e}"
    );
    // each with the runs that reach its stages, or it would not link
    let queue = "run Q (cost(Q, 1));";
    for (decl, runs) in [
        (
            format!("{QUEUE} queue D {{ pool kv {{ cap 1; }} serve fifo; }}"),
            format!("{queue} run D (cost(D, 1));"),
        ),
        (format!("pool kv {{ cap 1; }} {QUEUE}"), queue.to_string()),
        (
            format!("stage kv : fifo; {QUEUE}"),
            format!("{queue} run kv (cost(kv, 1));"),
        ),
        (
            "device gpu { kv cap 4; } pool kv on gpu { }".to_string(),
            String::new(),
        ),
        // nor a stage, which a schedule's `each at most` does not read
        (schedule_let("svc"), "Q (1, 1);".to_string()),
    ] {
        check(
            &SRC.replace("DECL", &decl)
                .replace("OBSERVE", &format!("{runs} observe response = 1;")),
            CLOSED,
        )
        .expect(&decl);
    }
}
