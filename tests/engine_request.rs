//! An engine's request (`docs/design/engine-request.md`): `request (…) {
//! … }` with its `at admission` and `cache`, and `taking` on `admit
//! waiting`, say in the engine what it does with one request, and the server calls it, `llm (prompt,
//! o);`. It is parse-time sugar for the hold a server writes, so a program
//! written either way has one IR; and what the design refuses does not link.

mod common;

use serq::compile_source;
use serq::ir::{CExpr, CStmt, CtxVar, Program};

fn compiled(src: &str) -> Program {
    compile_source(&common::main_source(src), &common::horizon(10.0))
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn refused(src: &str, fragment: &str) {
    match compile_source(&common::main_source(src), &common::horizon(10.0)) {
        Ok(_) => panic!("links:\n{src}"),
        Err(e) => assert!(e.contains(fragment), "missing {fragment:?}:\n{e}"),
    }
}

fn replaced(text: &str, pairs: &[(&str, &str)]) -> String {
    let mut t = text.to_string();
    for (a, b) in pairs {
        assert_eq!(t.matches(a).count(), 1, "{a}");
        t = t.replace(a, b);
    }
    t
}

const DEVICE: &str = "
let bs = 4;
device gpu { kv cap 64; }
";

const POOLS: &str = "
pool kv on gpu { block bs; evict lru; preempt lifo; }
pool reqs on llm { queue fifo; }
workload { arrive batch(3); hidden o; turn { set n = 6; set o = 3; } }
";

/// The engine says what it does with a request.
const ENGINE: &str = "
engine llm on gpu {
  reqs cap 2;
  tokens cap 8;
  request (prompt, output) {
    set kept = prompt;
    at admission { known = max(kept, computed + 1); }
    set kept = known;
    prefill (known - cached) growing kv;
    decode (output - 1 - (known - prompt)) growing kv;
    cache (prompt + output);
  }
  schedule {
    advance running;
    admit waiting while (running.preempted == 0)
      taking reqs (1), kv (min(known, budget_left(llm))) reserve (known);
  }
  execute (1);
}
";

const SERVER: &str = "
server {
  set t0 = now;
  set prompt = n;
  llm (prompt, o);
  observe response = now - t0;
}
";

/// The same, written as the server's hold.
const KERNEL: &str = "
engine llm on gpu {
  reqs cap 2;
  tokens cap 8;
  schedule { advance running; admit waiting while (running.preempted == 0); }
  execute (1);
}
pool kv on gpu { block bs; evict lru; preempt lifo; }
pool reqs on llm { queue fifo; }
workload { arrive batch(3); hidden o; turn { set n = 6; set o = 3; } }
server {
  set t0 = now;
  set prompt = n;
  set kept = prompt;
  hold reqs (cost(reqs, 1)), kv (cost(kv, min(known, budget_left(llm)))) reserve (cost(kv, known))
       at admission (known = max(kept, computed + 1)) {
    set kept = known;
    run llm prefill (cost(llm, known - cached)) growing kv;
    run llm decode (cost(llm, o - 1 - (known - prompt))) growing kv;
  } cache (cost(reqs, kv, prompt + o));
  observe response = now - t0;
}
";

fn program() -> String {
    format!("{DEVICE}{ENGINE}{POOLS}{SERVER}")
}

#[test]
fn a_request_is_the_hold_a_server_writes() {
    let sugar = compiled(&program());
    let kernel = compiled(&format!("{DEVICE}{KERNEL}"));
    assert_eq!(sugar.to_json(), kernel.to_json());
}

#[test]
fn a_mark_is_a_set_the_caller_reads_afterwards() {
    let src = replaced(
        &program(),
        &[
            (
                "prefill (known - cached) growing kv;",
                "prefill (known - cached) growing kv;\n    mark prefilled;",
            ),
            (
                "llm (prompt, o);",
                "llm (prompt, o);\n  observe prefill_done = llm.prefilled - t0;",
            ),
        ],
    );
    let p = compiled(&src);
    let slot = p
        .attrs
        .iter()
        .position(|a| a == "llm.prefilled")
        .expect("the mark is an attribute");
    let sets_now = |s: &CStmt| matches!(s, CStmt::Set(k, CExpr::Ctx(CtxVar::Now)) if *k == slot);
    assert!(
        p.blocks.iter().flatten().any(sets_now),
        "no `set llm.prefilled = now`"
    );
    assert!(p.observes.iter().any(|o| o == "prefill_done"));
}

#[test]
fn a_request_without_taking_holds_nothing() {
    let src = format!(
        "{DEVICE}
engine llm on gpu {{
  tokens cap 8;
  request (prompt) {{ prefill (prompt); }}
  schedule {{ advance running; admit waiting while (running.preempted == 0); }}
  execute (1);
}}
pool kv on gpu {{ block bs; }}
workload {{ arrive batch(1); turn {{ set n = 6; }} }}
server {{ llm (n); }}
"
    );
    let kernel = format!(
        "{DEVICE}
engine llm on gpu {{
  tokens cap 8;
  schedule {{ advance running; admit waiting while (running.preempted == 0); }}
  execute (1);
}}
pool kv on gpu {{ block bs; }}
workload {{ arrive batch(1); turn {{ set n = 6; }} }}
server {{ run llm prefill (cost(llm, n)); }}
"
    );
    assert_eq!(compiled(&src).to_json(), compiled(&kernel).to_json());
}

// What the request says, and where.

/// The request without its admission: no `at admission`, no `cache`.
const UNTAKEN: &[(&str, &str)] = &[
    (
        "\n      taking reqs (1), kv (min(known, budget_left(llm))) reserve (known)",
        "",
    ),
    (
        "    at admission { known = max(kept, computed + 1); }\n",
        "",
    ),
    ("    set kept = known;\n", ""),
    ("prefill (known - cached)", "prefill (prompt)"),
    (
        "decode (output - 1 - (known - prompt))",
        "decode (output - 1)",
    ),
    ("    cache (prompt + output);\n", ""),
];

#[test]
fn taking_needs_a_request() {
    let src = replaced(
        &program(),
        &[(
            "  request (prompt, output) {\n    set kept = prompt;\n    at admission { known = max(kept, computed + 1); }\n    set kept = known;\n    prefill (known - cached) growing kv;\n    decode (output - 1 - (known - prompt)) growing kv;\n    cache (prompt + output);\n  }\n",
            "",
        )],
    );
    refused(
        &src,
        "`taking` sizes the admission of engine `llm`'s request",
    );
}

#[test]
fn a_cache_needs_what_the_admission_took() {
    let mut pairs = UNTAKEN.to_vec();
    pairs.pop();
    refused(
        &replaced(&program(), &pairs),
        "`cache` in engine `llm`'s request, which takes nothing at admission",
    );
}

#[test]
fn bindings_need_an_admission() {
    let mut pairs = UNTAKEN.to_vec();
    pairs.remove(1);
    pairs.remove(1);
    pairs.remove(1);
    pairs.remove(1);
    let src = replaced(&program(), &pairs);
    refused(
        &src,
        "`at admission` in engine `llm`'s request, which takes nothing at admission",
    );
}

#[test]
fn every_admission_takes_the_same() {
    let src = replaced(
        &program(),
        &[(
            "    advance running;\n",
            "    advance running;\n    branch (running.count == 0) { admit waiting; }\n",
        )],
    );
    refused(&src, "write the same `taking` on each");
}

#[test]
fn taking_names_the_engine_s_pools() {
    let src = replaced(
        &program(),
        &[(
            "kv (min(known, budget_left(llm))) reserve (known)",
            "kvx (min(known, budget_left(llm))) reserve (known)",
        )],
    );
    refused(
        &src,
        "engine `llm`'s `taking` names `kvx`, which is no capacity of the engine or its device",
    );
}

#[test]
fn a_request_is_admitted_once() {
    let src = replaced(
        &program(),
        &[(
            "    set kept = known;\n",
            "    set kept = known;\n    at admission { late = known; }\n",
        )],
    );
    refused(&src, "a request is admitted once: one `at admission`");
}

#[test]
fn a_binding_goes_in_at_admission() {
    let src = replaced(
        &program(),
        &[(
            "    set kept = known;\n",
            "    set kept = known;\n    late = known;\n",
        )],
    );
    refused(
        &src,
        "an admission binding goes in the request's `at admission { … }`",
    );
}

#[test]
fn the_cache_is_the_request_s_last_item() {
    let src = replaced(
        &program(),
        &[(
            "    cache (prompt + output);\n",
            "    cache (prompt + output);\n    observe late = 1;\n",
        )],
    );
    refused(&src, "a request's `cache (e);` is its last item");
}

#[test]
fn the_admission_reads_no_attribute_of_the_session() {
    let src = replaced(
        &program(),
        &[("kv (min(known, budget_left(llm)))", "kv (min(known, n))")],
    );
    refused(
        &src,
        "engine `llm`'s admission (its `taking` and `at admission`) reads `n`",
    );
}

/// The admission reads the engine's own registers, as SGLang's reservation
/// reads its new-token ratio: the IR is the server hold that read it.
#[test]
fn the_admission_reads_the_engine_s_registers() {
    let state = [("tokens cap 8;", "tokens cap 8;\n  state ratio = 2;")];
    let sugar = compiled(&replaced(
        &program(),
        &[
            state[0],
            (
                "    advance running;\n",
                "    set ratio = ratio + 1;\n    advance running;\n",
            ),
            ("reserve (known);", "reserve (known + ratio);"),
        ],
    ));
    let kernel = compiled(&replaced(
        &format!("{DEVICE}{KERNEL}"),
        &[
            state[0],
            (
                "schedule { advance running;",
                "schedule { set ratio = ratio + 1; advance running;",
            ),
            (
                "reserve (cost(kv, known))",
                "reserve (cost(kv, known + ratio))",
            ),
        ],
    ));
    assert_eq!(sugar.to_json(), kernel.to_json());
}

#[test]
fn a_request_sets_no_parameter() {
    let src = replaced(
        &program(),
        &[(
            "    set kept = known;\n",
            "    set kept = known;\n    set prompt = known;\n",
        )],
    );
    refused(&src, "sets its parameter `prompt`");
}

#[test]
fn the_program_reads_what_the_request_marks_not_its_names() {
    let src = replaced(
        &program(),
        &[("observe response = now - t0;", "observe response = kept;")],
    );
    refused(&src, "`kept` is engine `llm`'s request's own");
}

/// Two engines' requests that name one value: one attribute would carry
/// both.
#[test]
fn two_requests_do_not_share_a_name() {
    let second = "
device gpu2 { kv2 cap 64; }
pool kv2 on gpu2 { block bs; }
engine llm2 on gpu2 {
  tokens cap 8;
  request (prompt) {
    set kept = prompt;
    at admission { }
    prefill (prompt) growing kv2;
  }
  schedule { advance running; admit waiting while (running.preempted == 0) taking kv2 (prompt); }
  execute (1);
}
";
    let src = format!("{DEVICE}{ENGINE}{POOLS}{second}{SERVER}");
    refused(
        &src,
        "`kept` is engine `llm`'s request's own and engine `llm2`'s too",
    );
}

#[test]
fn an_engine_has_one_request() {
    let src = replaced(
        &program(),
        &[(
            "  execute (1);\n",
            "  execute (1);\n  request (prompt) { prefill (prompt); }\n",
        )],
    );
    refused(&src, "`request` twice");
}

// The call.

#[test]
fn a_call_needs_the_engine_s_request() {
    let src = format!("{DEVICE}{KERNEL}");
    let src = replaced(
        &src,
        &[(
            "  observe response = now - t0;",
            "  llm (prompt, o);\n  observe response = now - t0;",
        )],
    );
    refused(&src, "engine `llm` has no `request (…) { … }`");
}

#[test]
fn a_call_takes_the_request_s_parameters() {
    let src = replaced(&program(), &[("llm (prompt, o);", "llm (prompt);")]);
    refused(&src, "takes 2 argument(s), got 1");
}

#[test]
fn an_argument_is_substituted_so_it_may_not_draw() {
    let src = replaced(
        &program(),
        &[("llm (prompt, o);", "llm (prompt, ~exp(3));")],
    );
    refused(&src, "draws a sample");
}

#[test]
fn a_session_does_not_call_an_engine() {
    let src = replaced(
        &program(),
        &[(
            "workload { arrive batch(3); hidden o; turn { set n = 6; set o = 3; } }",
            "workload { arrive batch(3); hidden o; turn { set n = 6; set o = 3; } session { turn; llm (n, o); } }",
        )],
    );
    refused(&src, "calls engine `llm`'s request, which a `server` does");
}

// Where a request may not be.

#[test]
fn a_queue_says_what_it_does_with_a_request_in_its_entries() {
    let src = "
let bs = 4;
queue P : prefill {
  device gpu { kv cap 64; }
  engine on gpu {
    tokens cap 8;
    request (prompt) { prefill (prompt); }
    schedule { advance running; admit waiting while (running.preempted == 0); }
    execute (1);
  }
  prefill (prompt) { run P prefill (cost(P, prompt)); }
}
workload { arrive batch(1); turn { set n = 6; } }
server { P.prefill (n); }
";
    refused(src, "a `request` in queue `P`'s engine");
}

#[test]
fn an_engine_family_has_no_request() {
    let src = "
device gpu[2] { kv cap 64; }
engine llm[2] on gpu {
  tokens cap 8;
  request (prompt) { prefill (prompt); }
  schedule { advance running; admit waiting while (running.preempted == 0); }
  execute (1);
}
workload { arrive batch(1); turn { set n = 6; } }
server { run llm[0] prefill (cost(llm, n)); }
";
    refused(src, "a `request` in engine family `llm`");
}
