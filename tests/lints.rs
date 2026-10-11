//! The two lints, and the corpus they must not fire on.
//!
//! Both come from bugs this repository shipped. Both are errors rather than
//! warnings because neither has a legitimate instance in `examples/` — a
//! warning nobody acts on is worse than no check — so a false positive here
//! is a real cost and `no_false_positives_on_the_corpus` is the test that
//! matters most.

mod common;

use serq::{Overrides, compile_source, program_path};

fn check(src: &str) -> Result<(), String> {
    compile_source(&common::main_source(src), &common::horizon(500.0)).map(|_| ())
}

/// Five lines, as `a_context_variable_outside_its_moment_is_rejected` reads a
/// line number below them.
const ENGINE: &str = "let bs = 16;
    device gpu { kv cap 1e5; } pool kv on gpu { block bs; evict lru; }
    pool reqs { cap 8; }
    engine llm on gpu { tokens cap 512; schedule { advance running; admit waiting while (running.preempted == 0); } execute (1e-3); }

    ";
const ENGINE_WORKLOAD: &str = "arrive poisson(0.3); init { set K = 0; }
               turn { set n = ~exp(500); set o = ~exp(200) + 1; }";

/// `examples/multi-turn/vllm.sq` shipped with a prefix-cache lookup bound before the
/// session queued, under a comment citing the admission-time lookup. A hold's
/// header is read at admission; a `set` above it is not.
#[test]
fn a_stale_header_read_is_rejected() {
    let src = format!(
        "{ENGINE} workload {{ {ENGINE_WORKLOAD} session {{ loop {{ turn; set K = prompt + o; end; }}
        }} }}
        server {{
          set prompt = K + n;
          set c = min(cachedin(kv), prompt - 1);
          hold reqs (cost(reqs, 1)), kv (cost(kv, c + min(prompt - c, budget_left(llm))))
          {{ run llm prefill (cost(llm, prompt - cached)) growing kv; }} cache (cost(reqs, kv, prompt + o));
        }}"
    );
    let e = check(&src).expect_err("rejected");
    assert!(e.contains("`c` reads cachedin(kv)"), "{e}");
    assert!(
        e.contains("hold on `kv`"),
        "names the pool that reads it: {e}"
    );
    assert!(
        e.contains("at admission"),
        "points at the clause for it: {e}"
    );
}

/// The same program with the clause links.
#[test]
fn at_admission_is_the_way_through() {
    let src = format!(
        "{ENGINE} workload {{ {ENGINE_WORKLOAD} session {{ loop {{ turn; set K = prompt + o; end; }}
        }} }}
        server {{
          set prompt = K + n;
          hold reqs (cost(reqs, 1)), kv (cost(kv, c + min(prompt - c, budget_left(llm))))
          at admission (c = min(cachedin(kv), prompt - 1))
          {{ run llm prefill (cost(llm, prompt - cached)) growing kv; }} cache (cost(reqs, kv, prompt + o));
        }}"
    );
    check(&src).expect("the clause is the way to say it");
}

/// Reassigning before the hold means the stale value never reaches it.
#[test]
fn a_reassignment_clears_the_lint() {
    let src = format!(
        "{ENGINE} workload {{ {ENGINE_WORKLOAD} session {{ loop {{ turn; set K = prompt + o; end; }}
        }} }}
        server {{
          set prompt = K + n;
          set c = min(cachedin(kv), prompt - 1);
          set c = 0;
          hold reqs (cost(reqs, 1)), kv (cost(kv, c + prompt)) {{ run llm prefill (cost(llm, prompt)) growing kv; }} cache (cost(reqs, kv, prompt));
        }}"
    );
    check(&src).expect("the read no longer reaches the header");
}

/// A `set` inside the body runs after admission, which is the timing it would
/// have had anyway.
#[test]
fn a_read_inside_the_body_is_fine() {
    let src = format!(
        "{ENGINE} workload {{ {ENGINE_WORKLOAD} session {{ loop {{ turn; set K = prompt + o; end; }}
        }} }}
        server {{
          set prompt = K + n;
          hold reqs (cost(reqs, 1)), kv (cost(kv, prompt)) {{
            set c = min(cachedin(kv), prompt - 1);
            observe hit = c; run llm prefill (cost(llm, prompt)) growing kv; }} cache (cost(reqs, kv, prompt));
        }}"
    );
    check(&src).expect("after admission is not stale");
}

/// A constant guard strictly between 0 and 1 is not a test; it was meant as
/// a draw, and the link error names the spelling for that.
#[test]
fn a_constant_probability_guard_is_rejected() {
    let src = "stage tool : delay;
        workload { arrive poisson(1); turn { set Z = ~exp(3); }
          session {  loop { branch (0.8) { turn;  } else { end; } }
          }
        }
        server { run tool (cost(tool, Z));
        }
        ";
    let e = check(src).expect_err("rejected");
    assert!(e.contains("`branch (0.8)` is not a test"), "{e}");
    assert!(e.contains("branch with (0.8)"), "{e}");
    // a constant that is neither 0 nor 1 and not a probability either
    let e = check(&src.replace("branch (0.8)", "branch (2)")).expect_err("rejected");
    assert!(
        e.contains("`branch (2)` is not a test: a guard is 0 or 1."),
        "{e}"
    );
    assert!(!e.contains("branch with"), "{e}");
    check(&src.replace("branch (0.8)", "branch (1)")).expect("0 and 1 are tests");
}

/// 0 and 1 are tests, not draws, and stay legal.
#[test]
fn zero_and_one_are_tests() {
    for g in ["0", "1"] {
        let src = format!(
            "stage tool : delay;
        workload {{ arrive poisson(1); turn {{ set Z = ~exp(3); }}
          session {{  loop {{ branch ({g}) {{ turn;  }} else {{ end; }} }}
          }}
        }}
        server {{ run tool (cost(tool, Z));
        }}
        "
        );
        check(&src).unwrap_or_else(|e| panic!("`branch ({g})` is a test: {e}"));
    }
}

/// A context variable outside the moment that supplies it. `tokens` is what a
/// step stage's budget and cost see for one iteration; in a session
/// statement it used to read as 0 and the program ran. The IR's validator
/// knows the table, and the linker now runs it on what it produces.
#[test]
fn a_context_variable_outside_its_moment_is_rejected() {
    let src = format!(
        "{ENGINE} workload {{ {ENGINE_WORKLOAD} session {{  turn; end;
        }} }}
        server {{ set x = tokens;
          hold reqs (cost(reqs, 1)), kv (cost(kv, n)) {{ run llm prefill (cost(llm, n)) growing kv; }}
        }}"
    );
    let e = check(&src).expect_err("rejected");
    // the block's role, after the statement's place in the text (#279)
    assert!(
        e.starts_with("9:26: session: "),
        "names the line and the block's role: {e}"
    );
    assert!(e.contains("set x = tokens;\n"), "shows the line: {e}");
    assert!(e.contains("`tokens` is read in a session statement"), "{e}");
    assert!(e.contains("exists only in an engine's `execute`"), "{e}");
    // every other moment refuses what it does not supply, and says where it
    // was read and where it exists
    const LLM: &str = "tokens cap 512; schedule { advance running; admit waiting while (running.preempted == 0); } execute (1e-3);";
    let engine = |pool: &str, llm: &str, session: &str| {
        format!(
            "let bs = 16;
        device gpu {{ kv cap 1e5; }}
        engine llm on gpu {{ {llm} }}
        pool kv on gpu {{ block bs; {pool} }}
        pool reqs {{ cap 8; }}
        stage svc : ps (4);
        workload {{ arrive poisson(0.3); init {{ set K = 0; }}
          turn {{ set m = ~exp(500); set o = ~exp(200) + 1; }}
          session {{  turn; end;
          }}
        }}

        server {{ {session} hold reqs (cost(reqs, 1)), kv (cost(kv, m)) {{ run llm prefill (cost(llm, m)) growing kv; }}
        }}"
        )
    };
    // (the prompt is `m`: a session attribute named `present` would shadow
    // the ps capacity variable)
    for (src, where_read, exists) in [
        (
            engine("evict by (tokens);", LLM, ""),
            "pool `kv`: `tokens` is read in an eviction key",
            "an engine's `execute`",
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
            "`batch.tokens` is read in `execute`",
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
            "`batch.attention` is read in `execute`",
        ),
        (
            engine("", &LLM.replace("execute (1e-3)", "execute (age)"), ""),
            "engine `llm`: `age` is read in an engine's `execute`",
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
            let src = engine("", LLM, "").replace("init { set K = 0; }", "init { set K = age; }");
            let e = check(&src).expect_err("rejected");
            assert!(e.contains(": init: `age`"), "{e}");
            continue;
        }
        let e = match check(&src) {
            Err(e) => e,
            Ok(()) => panic!("linked, but should have said `{where_read}`"),
        };
        assert!(e.contains(where_read), "want `{where_read}` in: {e}");
        assert!(e.contains(exists), "want `{exists}` in: {e}");
    }
    // what the table allows still links: `age`, `size` and a pool index in
    // an eviction key, `present` in a ps capacity, the residents' variables in a
    // `tokens cap` and an `each at most`, the batch's in `execute`, `now` at
    // every moment
    let src =
        "device gpu[2] { kv cap 1e5; }
        pool kv on gpu { evict by (age, size + used(kv[size > 1e9 ? 1 : 0]) * 0, now * 0); }
        stage svc : ps (min(present, 4) + now * 0);
        engine llm[2] on gpu {
          tokens cap 512 + running.count + running.decoding + running.kv_decode * 0 + running.kv_prefill * 0 + now * 0;
          schedule {
            let c = running.decoding > 0 ? 64 : 128;
            advance running each at most (c);
            admit waiting while (running.preempted == 0) each at most (c);
          }
          execute (1e-3 * batch.tokens + batch.prefilled * 0 + batch.attention * 0 + now * 0);
        }
        workload { arrive poisson(0.3); turn { set n = ~exp(500); }
          session {  turn; end;
          }
        }
        server { set t = now; hold kv[0] (cost(kv, n)) { run svc (cost(svc, n)); }
        }
        ";
    check(src).expect("links");
}

/// A program whose engine `llm` serves its residents by `advance`, a
/// schedule statement, and then admits as vLLM does.
fn advancing(advance: &str) -> String {
    format!(
        "device gpu {{ kv cap 1e5; }}
        engine llm on gpu {{ tokens cap 512; schedule {{ {advance} admit waiting while (running.preempted == 0); }} execute (1); }}
        pool kv on gpu {{ }}
        workload {{ arrive batch(1);
          session {{ turn; end;
          }}
        }}
        server {{ hold kv (cost(kv, 1)) {{ run llm prefill (cost(llm, 1)) growing kv; }}
        }}
        "
    )
}

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
        check(&advancing(ok)).unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
    let e = check(&advancing("advance running shortest;")).expect_err("unknown");
    assert!(e.contains("found `shortest`"), "{e}");
}

/// `advance running admission` is `by` with no keys (every resident ties, and
/// ties are admission order), so the IR knows one form; a serve key may read
/// the residents' variables and may not draw.
#[test]
fn serve_admission_is_by_with_no_keys_and_a_key_does_not_draw() {
    let ir = |s: &str| {
        serq::compile_source(&common::main_source(&advancing(s)), &common::horizon(10.0))
            .unwrap()
            .to_json()
    };
    assert_eq!(ir("advance running admission;"), ir("advance running;"));
    assert!(ir("advance running admission;").contains("\"By\": []"));
    check(&advancing(
        "advance running by (running.count > 4 ? -remaining : admission);",
    ))
    .expect("the residents' variables are keys");
    let e = check(&advancing("advance running by (~uniform(0, 1));")).expect_err("a draw");
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
    let wl = "let bs = 16;
        device gpu { kv cap 1e5; }
        engine llm on gpu {
          tokens cap 512;
          schedule { advance running; admit waiting while (running.preempted == 0); }
          execute (1e-3);
        }
        pool kv on gpu { block bs; evict lru; }
        pool reqs { cap 8; }

        ";
    let wl_workload = "arrive poisson(0.3); hidden o; init { set K = 0; }
                   turn { set n = ~exp(500); set o = ~exp(200) + 1; }";
    // the body may read it
    let ok = format!(
        "{wl} workload {{ {wl_workload} session {{  turn; end; \n}} }}\nserver {{ hold reqs (cost(reqs, 1)), kv (cost(kv, n)) {{ run llm prefill (cost(llm, n)) growing kv; run llm decode (cost(llm, o - 1)) growing kv; }} cache (cost(reqs, kv, n + o));\n}}"
    );
    check(&ok).expect("links");
    // a hold's header may not: the reservation is the scheduler's
    let bad = format!(
        "{wl} workload {{ {wl_workload} session {{  turn; end; \n}} }}\nserver {{ hold reqs (cost(reqs, 1)), kv (cost(kv, n)) reserve (cost(kv, n + o)) {{ run llm prefill (cost(llm, n)) growing kv; }}\n}}"
    );
    let e = check(&bad).expect_err("rejected");
    assert!(e.contains("`o` is hidden from the scheduler"), "{e}");
    assert!(e.contains("read at admission"), "{e}");
    // the server-block spelling substitutes the binding into the header,
    // and the check sees the substituted expression
    let bad = format!(
        "{wl} workload {{ {wl_workload} session {{  turn; end; }} }}
        server {{ hold reqs (cost(reqs, 1)), kv (cost(kv, n)) reserve (cost(kv, need)) at admission (need = n + o) {{ run llm prefill (cost(llm, n)) growing kv; }} }}"
    );
    let e = check(&bad).expect_err("rejected");
    assert!(e.contains("`o` is hidden from the scheduler"), "{e}");
    assert!(e.contains("read at admission"), "{e}");
    // nor a pool's queue key
    let bad = "let bs = 16;
        device gpu { kv cap 1e5; }
        engine llm on gpu {
          tokens cap 512;
          schedule { advance running; admit waiting while (running.preempted == 0); }
          execute (1e-3);
        }
        pool kv on gpu { block bs; evict lru; queue by (o); }
        workload { arrive poisson(0.3); hidden o; turn { set n = ~exp(500); set o = ~exp(200) + 1; }
          session {  turn; end;
          }
        }

        server { hold kv (cost(kv, n)) { run llm prefill (cost(llm, n)) growing kv; }
        }";
    let e = check(bad).expect_err("rejected");
    assert!(e.contains("pool `kv`"), "{e}");
    assert!(e.contains("`o` is hidden from the scheduler"), "{e}");
    // nor an engine's serve key, read for each running request
    let bad = "let bs = 16;
        device gpu { kv cap 1e5; }
        engine llm on gpu {
          tokens cap 512;
          schedule { advance running by (o); admit waiting while (running.preempted == 0); }
          execute (1e-3);
        }
        pool kv on gpu { block bs; evict lru; }
        workload { arrive poisson(0.3); hidden o; turn { set n = ~exp(500); set o = ~exp(200) + 1; }
          session {  turn; end;
          }
        }

        server { hold kv (cost(kv, n)) { run llm prefill (cost(llm, n)) growing kv; }
        }";
    let e = check(bad).expect_err("rejected");
    assert!(e.contains("engine `llm`"), "{e}");
    assert!(e.contains("`o` is hidden from the scheduler"), "{e}");
    // a name nothing sets is not an attribute
    let bad = "pool kv { cap 1e5; }
        stage svc : fifo;
        workload { arrive poisson(0.3); hidden nothing;
          session { turn; end;
          }
        }

        server { hold kv (cost(kv, 1)) { run svc (cost(svc, 1)); }
        }";
    let e = check(bad).expect_err("rejected");
    assert!(e.contains("hidden `nothing`"), "{e}");
    // what the scheduler sets cannot be hidden from it, and a name is hidden once
    let bad = format!("{wl} workload {{ {wl_workload} session {{  turn; end; \n}} }}\nserver {{ hold kv (cost(kv, n)) {{ run llm prefill (cost(llm, n)) growing kv; }}\n}}")
        .replace("hidden o;", "hidden computed;");
    let e = check(&bad).expect_err("rejected");
    assert!(
        e.contains("hidden `computed`: the scheduler sets it"),
        "{e}"
    );
    let bad = format!("{wl} workload {{ {wl_workload} session {{  turn; end; \n}} }}\nserver {{ hold kv (cost(kv, n)) {{ run llm prefill (cost(llm, n)) growing kv; }}\n}}")
        .replace("hidden o;", "hidden o, o;");
    let e = check(&bad).expect_err("rejected");
    assert!(e.contains("hidden `o` twice"), "{e}");
}

/// A stage's capacity, budget and cost, and an iteration body's guard, are
/// read for the stage, where no one session is. `ps(1 + prompt)` linked and
/// read the attribute as NaN: the run ended no session and said nothing.
#[test]
fn a_stage_reads_no_session_attribute() {
    let ps = "stage svc : ps(1 + prompt);
        workload { arrive poisson(0.5); init { set prompt = 2; } }
        server { run svc (cost(svc, 1)); }";
    let e = check(ps).expect_err("rejected");
    assert!(e.contains("`prompt` is a session attribute"), "{e}");
    assert!(e.contains("a ps stage's capacity"), "{e}");
    let engine = |tokens: &str, schedule: &str, execute: &str| {
        format!(
            "device gpu {{ kv cap 100; }}
            engine llm on gpu {{
              tokens cap {tokens};
              schedule {{ {schedule} }}
              execute ({execute});
            }}
            pool kv on gpu {{ }}
            workload {{ arrive poisson(0.5); init {{ set prompt = 2; }} }}
            server {{ hold kv (cost(kv, prompt)) {{ run llm prefill (cost(llm, prompt)) growing kv; }} }}"
        )
    };
    let vllm = "advance running; admit waiting while (running.preempted == 0);";
    check(&engine("8", vllm, "1")).expect("links");
    for (src, moment) in [
        (engine("8 + prompt", vllm, "1"), "an engine's `tokens cap`"),
        (engine("8", vllm, "1 + prompt"), "an engine's `execute`"),
        (
            engine(
                "8",
                "branch (prompt > 0) { advance running; } admit waiting while (running.preempted == 0);",
                "1",
            ),
            "an engine's `schedule`",
        ),
    ] {
        let e = check(&src).expect_err("rejected");
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
        let src = std::fs::read_to_string(&path).unwrap();
        serq::compile_source_at(
            &common::main_source(&src),
            path.parent(),
            &common::horizon(500.0),
        )
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
            "device gpu {{ kv cap 10; }}\nengine e on gpu {{ tokens cap inf; schedule {{ advance running; admit waiting while (running.preempted == 0); }} execute ({cost}); }}\npool kv on gpu {{ evict by ({key}); }}\nworkload {{ session {{ turn; end; \n}} }}\nserver {{\n}}\n"
        );
        let e = compile_source(&common::main_source(&src), &common::horizon(500.0)).unwrap_err();
        assert!(e.contains(says), "{e}");
    }
}

const CACHE_ENGINE: &str = "device gpu { kv cap 1e5; }
    engine llm on gpu {
      tokens cap 512;
      schedule { advance running; admit waiting while (running.preempted == 0); }
      execute (1e-3);
    }
    pool kv on gpu { block 16; evict lru; }
    stage think : delay;
    ";
const CACHE_ENGINE_WORKLOAD: &str = "arrive closed(1);";

/// #230: a hold without a `cache` clause consumes nothing of the session's
/// prefix, so `cached` is 0 in its body; reading it there is the old idiom
/// for a probe that consumed and kept nothing, which is now `cache (0)`.
#[test]
fn cached_in_a_hold_without_cache_is_rejected() {
    let src = format!(
        "{CACHE_ENGINE} workload {{ {CACHE_ENGINE_WORKLOAD} session {{ turn;
        }} }}
        server {{ loop {{ run think (cost(think, 1));
            hold kv (cost(kv, 1000)) {{ observe hit = cached > 0; run llm prefill (cost(llm, 1000 - cached)) growing kv; }}
          }}
        }} "
    );
    let e = check(&src).expect_err("rejected");
    assert!(
        e.contains("`cached` is read in a hold on `kv` that has no `cache` clause"),
        "{e}"
    );
    assert!(e.contains("`cache (0)`"), "names the spelling for it: {e}");
}

/// `cache (0)` is that spelling, and the usual request links as before.
#[test]
fn cache_zero_is_the_way_through() {
    for clause in ["cache (cost(kv, 0))", "cache (cost(kv, 1000))"] {
        let src = format!(
            "{CACHE_ENGINE} workload {{ {CACHE_ENGINE_WORKLOAD} session {{ turn;
        }} }}
        server {{ loop {{ run think (cost(think, 1));
            hold kv (cost(kv, 1000)) {{ observe hit = cached > 0; run llm prefill (cost(llm, 1000 - cached)) growing kv; }} {clause};
          }}
        }} "
        );
        check(&src).unwrap_or_else(|e| panic!("{clause}: {e}"));
    }
}

/// The innermost hold is the admission that set `cached`: a hold on
/// another pool nested in the request's reads its own, which is 0.
#[test]
fn cached_in_a_nested_hold_on_another_pool_is_rejected() {
    let src = format!(
        "{CACHE_ENGINE} pool reqs {{ cap 4; }} workload {{ {CACHE_ENGINE_WORKLOAD} session {{ turn;
        }} }}
        server {{ loop {{ run think (cost(think, 1));
            hold kv (cost(kv, 1000)) {{ hold reqs (cost(reqs, 1)) {{ run llm prefill (cost(llm, 1000 - cached)) growing kv; }} }} cache (cost(kv, 1000));
          }}
        }} "
    );
    let e = check(&src).expect_err("rejected");
    assert!(e.contains("hold on `reqs`"), "{e}");
    let src = format!(
        "{CACHE_ENGINE} pool reqs {{ cap 4; }} workload {{ {CACHE_ENGINE_WORKLOAD} session {{ turn;
        }} }}
        server {{ loop {{ run think (cost(think, 1));
            hold kv (cost(kv, 1000)) {{ set c = cached; hold reqs (cost(reqs, 1)) {{ run llm prefill (cost(llm, 1000 - c)) growing kv; }} }} cache (cost(kv, 1000));
          }}
        }} "
    );
    check(&src).expect("read above the inner hold, as llmd_nixl_pull.sq does");
    // read after the inner hold, in the outer body: the inner admission set
    // it, so `c` was 0 every turn where 992 was kept (#237); in a branch
    // after it, and in a loop whose next iteration comes after it
    let inner = "hold reqs (cost(reqs, 1)) { run think (cost(think, 0.01)); }";
    for body in [
        format!("{inner} observe c = cached;"),
        format!("{inner} branch (1 > 0) {{ observe c = cached; }}"),
        format!("loop {{ observe c = cached; {inner} }}"),
    ] {
        let src = format!(
            "{CACHE_ENGINE} pool reqs {{ cap 4; }} workload {{ {CACHE_ENGINE_WORKLOAD} session {{ turn;
            }} }}
            server {{ loop {{ run think (cost(think, 1));
                hold kv (cost(kv, 1000)) {{ {body} }} cache (cost(kv, 1000));
              }}
            }} "
        );
        let e = check(&src).expect_err("rejected");
        assert!(
            e.contains("`cached` is read after the hold on `reqs` in the body of the hold on `kv`"),
            "{body}\n{e}"
        );
    }
}

/// `reuse` bounds what the admission consumes; without `cache` it consumes nothing.
#[test]
fn reuse_without_cache_is_rejected() {
    let src = format!(
        "{CACHE_ENGINE} workload {{ {CACHE_ENGINE_WORKLOAD} session {{ turn;
        }} }}
        server {{ loop {{ run think (cost(think, 1));
            hold kv (cost(kv, 1000)) reuse (cost(kv, 512)) {{ run llm prefill (cost(llm, 1000)) growing kv; }}
          }}
        }} "
    );
    let e = check(&src).expect_err("rejected");
    assert!(
        e.contains("`reuse` on a hold on `kv` that has no `cache` clause"),
        "{e}"
    );
}

/// #231: a name resolves to an attribute first, then a `let`, then a context
/// variable, so `set present = 500;` anywhere made `ps(min(present, 16))`
/// read the attribute: capacity 16 for 2 jobs, a service time of 0.125
/// where the jobs present give 1. Neither an attribute nor a `let` may take
/// a name the language supplies (a context variable, `inf`).
#[test]
fn a_context_variable_name_cannot_be_an_attribute_or_a_constant() {
    const PS: &str = "stage dec : ps(min(present, 16));
        stage think : delay;
        workload { arrive closed(2);
          session { turn;
          }
        }
        server { loop { SET run dec (cost(dec, 1)); observe r = now; run think (cost(think, 1)); }
        }
        ";
    let e = check(&PS.replace("SET", "set present = 500;")).expect_err("rejected");
    assert!(
        e.contains("`present` is a name the language supplies, read in a ps stage's capacity"),
        "{e}"
    );
    assert!(e.contains("a session attribute named `present`"), "{e}");
    let e = check(&format!("let present = 500; {}", PS.replace("SET", ""))).expect_err("rejected");
    assert!(e.contains("a `let` constant named `present`"), "{e}");
    // `inf` is folded at parse time, so the answer would be right, but the
    // name is the language's, as it is for a binding or an aggregate's index.
    let e = check(&PS.replace("SET", "set inf = 500;")).expect_err("rejected");
    assert!(
        e.contains("`inf` is a name the language supplies, read in every expression"),
        "{e}"
    );
    // Renamed, the program links, and the capacity reads the jobs present:
    // `ps(φ)` serves each job at `φ(present)/present` = 1 while at most 16
    // are present, so a unit of work takes exactly 1 s whatever the arrivals
    // (the attribute's 0.125 needs the two jobs together: 16/2 = 8).
    let src = PS.replace("SET", "set prompt = 500;");
    check(&src).expect("links");
    let r = serq::run_source(
        &common::main_source(&src),
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            ..common::horizon(50.0)
        },
        None,
    )
    .unwrap();
    assert_eq!(r.stage("dec").unwrap().mean_service, 1.0, "{}", r.text());
}
