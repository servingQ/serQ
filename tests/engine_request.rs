//! An engine's request (`docs/design/scheduler-worker.md`, step 3): the
//! engine keeps a request's state as vLLM does; what its admission takes is
//! `taking … [reuse (e)] [prefix (…)] [cache (e)]` on `admit waiting`, read
//! as `request.…`, and the server calls it, `llm (prompt, o);`. A request
//! runs as the hold a queue's entry writes in the kernel, and what the
//! design refuses does not link.

mod common;

use serq::ir::{CExpr, CStageKind, Program};

/// Three requests of a 6-token prompt that generate 3: the workload of
/// most tests here, the server's `n` and its hidden `o`.
const WORKLOAD: &str = "workload { arrive batch(3); turn { set n = 6; set o = 3; } send (n, o); }";

fn compiled(src: &str) -> Program {
    compiled_with(src, WORKLOAD)
}

fn compiled_with(src: &str, workload: &str) -> Program {
    common::compile(&common::main_source(src), workload, &common::horizon(10.0))
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn run(src: &str) -> serq::Report {
    run_with(src, WORKLOAD)
}

fn run_with(src: &str, workload: &str) -> serq::Report {
    common::run(&common::main_source(src), workload, &common::horizon(100.0))
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn refused(src: &str, fragment: &str) {
    refused_with(src, WORKLOAD, fragment)
}

fn refused_with(src: &str, workload: &str, fragment: &str) {
    match common::compile(&common::main_source(src), workload, &common::horizon(10.0)) {
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
";

/// The engine admits a request with a slot and the blocks of the chunk the
/// budget allows, if every token it has fits, and leaves it cached.
const ENGINE: &str = "
engine llm on gpu {
  reqs cap 2;
  tokens cap 8;
  scheduler {
    advance running;
    admit waiting while (running.preempted == 0)
      taking reqs (1), kv (min(request.known, budget_left(llm))) reserve (request.known)
      cache (request.input + request.output);
  }
  worker { forward (1); }
}
";

const SERVER: &str = "
server (n, hidden o) {
  set t0 = now;
  llm (n, o);
  observe response = now - t0;
}
";

/// The same, written as the hold of a queue's entry, the one place the
/// kernel spelling of an engine's admission is still written: vLLM's
/// request, the tokens it has (`known`) never fewer than an earlier attempt
/// had (`kept`).
const KERNEL: &str = "
let bs = 4;
queue llm : prefill {
  device gpu { kv cap 64; }
  engine on gpu {
    reqs cap 2;
    tokens cap 8;
    scheduler { advance running; admit waiting while (running.preempted == 0); }
    worker { forward (1); }
  }
  pool kv on gpu { block bs; evict lru; preempt lifo; }
  pool reqs on llm { queue fifo; }
  prefill (prompt) {
    set kept = prompt;
    hold reqs (cost(reqs, 1)),
         kv (cost(kv, min(max(kept, computed + 1), budget_left(llm))))
         reserve (cost(kv, max(kept, computed + 1))) {
      set known = max(kept, computed + 1);
      set kept = known;
      run llm prefill (cost(llm, known - cached)) growing kv;
      run llm decode (cost(llm, o - 1 - (known - prompt))) growing kv;
    } cache (cost(reqs, kv, prompt + o));
  }
}
server (n, hidden o) {
  set t0 = now;
  llm.prefill (n);
  observe response = now - t0;
}
";

fn program() -> String {
    format!("{DEVICE}{ENGINE}{POOLS}{SERVER}")
}

#[test]
fn a_request_runs_as_the_kernel_hold_does() {
    let request = run(&program());
    let kernel = run(KERNEL);
    let samples = |r: &serq::Report| r.observe("response").unwrap().samples.clone();
    assert_eq!(samples(&request), samples(&kernel));
    let pools = |r: &serq::Report| {
        let kv = r.pool("kv").or_else(|| r.pool("llm.kv")).unwrap();
        (kv.admissions, kv.preemptions)
    };
    assert_eq!(pools(&request), pools(&kernel));
}

/// The engine records each admission: the cached tokens it read and the
/// tokens its prefill computes; the caller reads when the request was
/// admitted and when its first token came.
#[test]
fn the_engine_records_its_requests() {
    let src = replaced(
        &program(),
        &[(
            "  observe response = now - t0;",
            "  observe response = now - t0;\n  observe first = llm.first - t0;\n  observe admitted_at = llm.admitted - t0;\n  observe read_and_computed = llm.reused + llm.prefill;",
        )],
    );
    let r = run(&src);
    let admissions = r.pool("kv").unwrap().admissions;
    assert_eq!(r.observe("llm.prefill").unwrap().count, admissions);
    assert_eq!(r.observe("llm.reused").unwrap().count, admissions);
    // the latest admission's, as attributes: what it read and computes is
    // every token the request had then, the prompt `n` (6) when unpreempted
    let last = &r.observe("read_and_computed").unwrap().samples;
    assert!(last.iter().all(|&t| t >= 6.0), "{last:?}");
    let first = &r.observe("first").unwrap().samples;
    let admitted = &r.observe("admitted_at").unwrap().samples;
    let response = &r.observe("response").unwrap().samples;
    for k in 0..first.len() {
        assert!(admitted[k] < first[k] && first[k] <= response[k], "{k}");
    }
    refused(
        &replaced(&program(), &[("now - t0;", "llm.prefilled - t0;")]),
        "`llm.prefilled`",
    );
}

/// A request of no input decodes from its first iteration: `o` tokens,
/// one per iteration.
#[test]
fn a_request_without_input_decodes_at_once() {
    let src = replaced(&program(), &[("llm (n, o);", "llm (0, o);")]);
    let r = run(&src);
    assert_eq!(r.observe("response").unwrap().samples[0], 3.0);
}

#[test]
fn a_request_without_taking_holds_nothing() {
    let src = format!(
        "{DEVICE}
engine llm on gpu {{
  tokens cap 8;
  scheduler {{ advance running; admit waiting while (running.preempted == 0); }}
  worker {{ forward (1); }}
}}
pool kv on gpu {{ block bs; }}
server (n) {{ llm (n, 1); }}
"
    );
    let workload = "workload { arrive batch(1); turn { set n = 6; } send (n); }";
    let p = compiled_with(&src, workload);
    let CStageKind::Step(st) = &p.stages[0].kind else {
        panic!("the engine")
    };
    assert!(st.request.as_ref().unwrap().pools.is_empty());
    assert_eq!(run_with(&src, workload).pool("kv").unwrap().admissions, 0);
}

/// The cache may read whether the request has finished: a retracted one
/// leaves what `cache` says of it, a finished one all of it (SGLang's
/// `cache (request.finished ? … : request.input)`, which no run of
/// `sglang.sq` preempts). Two requests of 16 tokens that generate 24 outgrow
/// four blocks: the second is preempted with two blocks computed, the first
/// takes one of them, and the second, admitted again when the first ends,
/// reads the other back if its preemption cached it, and nothing if
/// `request.finished` was 0 there; at its end it caches its 40 tokens'
/// two full blocks.
#[test]
fn the_cache_reads_whether_the_request_finished() {
    let with = |cache: &str| {
        format!(
            "device gpu {{ kv cap 64; }}
             pool kv on gpu {{ block 16; evict lru; preempt lifo; }}
             engine e on gpu {{
               tokens cap 64;
               scheduler {{
                 advance running;
                 admit waiting while (running.preempted == 0)
                   taking kv (request.known) reuse (floor(request.known - 1, 16)) cache ({cache});
               }}
               worker {{ forward (1); }}
             }}
             server {{ e (16, 24); observe after = cachedin(kv); }}"
        )
    };
    let reused = |cache: &str| {
        let r = run_with(&with(cache), "workload { arrive batch(2); }");
        assert_eq!(r.pool("kv").unwrap().preemptions, 1, "{}", r.text());
        (
            r.observe("e.reused").unwrap().samples.clone(),
            r.observe("after").unwrap().samples.clone(),
        )
    };
    assert_eq!(reused("request.input + request.output").0, [0.0, 0.0, 16.0]);
    let (again, after) = reused("request.finished ? request.input + request.output : 0");
    assert_eq!(again, [0.0, 0.0, 0.0]);
    assert_eq!(after.last(), Some(&32.0));
}

/// The admission reads the engine's own registers, as SGLang's reservation
/// reads its new-token ratio.
#[test]
fn the_admission_reads_the_engine_s_registers() {
    let p = compiled(&replaced(
        &program(),
        &[
            ("tokens cap 8;", "tokens cap 8;\n  state ratio = 2;"),
            (
                "    advance running;\n",
                "    set ratio = ratio + 1;\n    advance running;\n",
            ),
            ("reserve (request.known)", "reserve (request.known + ratio)"),
        ],
    ));
    let CStageKind::Step(st) = &p.stages[0].kind else {
        panic!("the engine")
    };
    let reserve = st.request.as_ref().unwrap().pools[1].2.as_ref().unwrap();
    assert!(reserve.any(&|e| matches!(e, CExpr::Reg(0))), "{reserve:?}");
}

/// Only the admission reads a register: the server reads none, and an
/// admission that waits at a pool the engine does not admit is admitted
/// apart from the engine's iteration.
#[test]
fn a_register_is_read_by_the_admission_alone() {
    let with_ratio = |more: &[(&str, &str)]| {
        let mut pairs = vec![
            ("tokens cap 8;", "tokens cap 8;\n  state ratio = 2;"),
            (
                "    advance running;\n",
                "    set ratio = ratio + 1;\n    advance running;\n",
            ),
        ];
        pairs.extend_from_slice(more);
        replaced(&program(), &pairs)
    };
    refused(
        &with_ratio(&[(
            "  observe response = now - t0;",
            "  observe response = now - t0;\n  observe r = ratio;",
        )]),
        "`ratio` is an engine's register (`state`), which its schedule reads",
    );
    refused(
        &with_ratio(&[(
            "taking reqs (1), kv (min(request.known, budget_left(llm))) reserve (request.known)",
            "taking kv (min(request.known, budget_left(llm))) reserve (request.known + ratio), reqs (1)",
        )]),
        "`ratio` is engine `llm`'s register",
    );
    // the release is the session's, as a hold's `cache` is (#592 review)
    refused(
        &with_ratio(&[(
            "cache (request.input + request.output);",
            "cache (request.input + request.output + ratio);",
        )]),
        "`ratio` is an engine's register (`state`), which its schedule reads; it is read in \
         an engine's `cache` or a connector's `save`",
    );
}

/// A request generates at least one token, as vLLM's `max_tokens` is at
/// least 1 (sampling_params.py:612-614): a constant 0 does not link, and an
/// output that comes to 0 stops the run (#592 review: it sampled a token).
#[test]
fn a_request_generates_a_token() {
    refused(
        &replaced(&program(), &[("  llm (n, o);", "  llm (n, 0);")]),
        "a request to `llm` generates 0: a request generates at least one token",
    );
    let src = replaced(
        &program(),
        &[("  llm (n, o);", "  set z = 0;\n  llm (n, z);")],
    );
    let e = common::run(
        &common::main_source(&src),
        WORKLOAD,
        &common::horizon(100.0),
    )
    .expect_err("a request of no token runs");
    assert!(e.contains("a request generates at least one token"), "{e}");
}

// What the engine writes, and where.

#[test]
fn an_engine_has_no_request_block() {
    let src = replaced(
        &program(),
        &[(
            "  worker { forward (1); }\n",
            "  worker { forward (1); }\n  request (prompt) { prefill (prompt); }\n",
        )],
    );
    refused(&src, "an engine keeps a request's state");
}

#[test]
fn every_admission_takes_the_same() {
    let src = replaced(
        &program(),
        &[(
            "    advance running;\n",
            "    advance running;\n    if (running.count == 0) { admit waiting; }\n",
        )],
    );
    refused(&src, "write the same `taking` on each");
}

#[test]
fn taking_names_the_engine_s_pools() {
    let src = replaced(
        &program(),
        &[(
            "kv (min(request.known, budget_left(llm)))",
            "kvx (min(request.known, budget_left(llm)))",
        )],
    );
    refused(
        &src,
        "engine `llm`'s `taking` names `kvx`, which is no capacity of the engine or its device",
    );
    refused(
        &replaced(
            &program(),
            &[(
                "taking reqs (1), kv (min(request.known, budget_left(llm))) reserve (request.known)\n      cache",
                "taking cache",
            )],
        ),
        "`taking` names the pools the admission takes first",
    );
}

/// The request's names are read where the request has them: `hit` at its
/// admission, `finished` at its release, none outside them, and `taking`
/// reads no total of the engine's.
#[test]
fn the_request_s_names_are_read_where_it_has_them() {
    let at = |from: &str, to: &str, said: &str| {
        refused(&replaced(&program(), &[(from, to)]), said);
    };
    at(
        "cache (request.input + request.output)",
        "cache (request.hit)",
        "`request.hit` is read at the request's admission",
    );
    at(
        "reserve (request.known)",
        "reserve (request.finished)",
        "`request.finished` is read when the request gives its pools back",
    );
    at(
        "forward (1)",
        "forward (request.known)",
        "`request.known` is the request's",
    );
    at(
        "reserve (request.known)",
        "reserve (running.count)",
        "`taking` is read for one request",
    );
    at(
        "reserve (request.known)",
        "reserve (request.size)",
        "`request.size`",
    );
}

// The call.

#[test]
fn a_call_is_an_input_and_an_output() {
    let src = replaced(&program(), &[("llm (n, o);", "llm (n, o, 1);")]);
    refused(
        &src,
        "a request is its prompt's tokens and the tokens it generates",
    );
}

/// The server runs an engine only through its request: a run of its
/// prefill or decode, and a hold of a pool it admits (an engine capacity, or
/// a pool `on E.gpu`), are refused. A hold of a pool no engine admits stays
/// the server's, around the call.
#[test]
fn the_server_runs_an_engine_only_through_its_request() {
    let call = "  llm (n, o);\n";
    let run = replaced(
        &program(),
        &[(call, "  llm (n, o);\n  run llm decode (cost(llm, 1));\n")],
    );
    refused(
        &run,
        "the server runs engine `llm`'s decode (`run llm decode (…)`): an engine runs only in its \
         own queue's entries",
    );
    let held = replaced(
        &program(),
        &[(call, "  hold reqs (cost(reqs, 1)) { llm (n, o); }\n")],
    );
    refused(&held, "the server holds `reqs`, which engine `llm` admits");
    let on_engine_device = replaced(
        &program(),
        &[
            (
                "pool kv on gpu { block bs; evict lru; preempt lifo; }",
                "pool kv on llm.gpu { block bs; evict lru; preempt lifo; }",
            ),
            (call, "  hold kv (cost(kv, 1)) { llm (n, o); }\n"),
        ],
    );
    refused(
        &on_engine_device,
        "the server holds `kv`, which engine `llm` admits",
    );
    let gated = replaced(
        &program(),
        &[
            (
                "pool reqs on llm { queue fifo; }",
                "pool reqs on llm { queue fifo; }\npool gate { cap 1; }",
            ),
            (call, "  hold gate (cost(gate, 1)) { llm (n, o); }\n"),
        ],
    );
    compiled(&gated);
}

#[test]
fn a_session_does_not_call_an_engine() {
    let workload = replaced(
        WORKLOAD,
        &[(
            "send (n, o); }",
            "send (n, o); session { turn; llm (n, o); } }",
        )],
    );
    refused_with(
        &program(),
        &workload,
        "is a request to engine `llm`, which a `server` makes",
    );
    // a queue's entry runs its own engine, and no other
    refused(
        &replaced(
            &program(),
            &[(
                "pool reqs on llm { queue fifo; }\n",
                "pool reqs on llm { queue fifo; }\n\
                 queue Q : prefill { pool slots { cap 2; } serve fifo; \
                 prefill (p) { hold slots (cost(slots, 1)) { run llm prefill (cost(llm, p)); } } }\n",
            )],
        ),
        "`Q.prefill` runs engine `llm`'s prefill",
    );
}

// Where a request may not be.

/// A queue's engine admits by its `taking`, as any engine, and its entries
/// call it, `P (prompt, 1);`, a family's member its own pools; the server
/// calls the entry, not the engine (#592).
#[test]
fn a_queue_s_entries_call_its_engine() {
    let queue = |entry: &str, server: &str| {
        format!(
            "
let bs = 4;
queue P[2] : prefill {{
  device gpu {{ kv cap 64; }}
  engine on gpu {{
    tokens cap 8;
    scheduler {{ advance running; admit waiting taking kv (request.known); }}
    worker {{ forward (1); }}
  }}
  pool kv on gpu {{ }}
  prefill (prompt) {{ {entry} }}
}}
server (n) {{ {server} }}
"
        )
    };
    let workload = "workload { arrive batch(2); turn { set n = 6; } send (n); }";
    let r = run_with(
        &queue(
            "P (prompt, 1); observe read = P.prefill;",
            "P[serial].prefill (n);",
        ),
        workload,
    );
    assert_eq!(r.observe("read").unwrap().samples, [6.0, 6.0]);
    // each member admitted into its own pool
    let members: Vec<u64> = r.pools_named("P.kv").iter().map(|p| p.admissions).collect();
    assert_eq!(members, [1, 1], "{}", r.text());
    refused_with(
        &queue("P (prompt, 1);", "P (n, 1);"),
        workload,
        "`P (…)` is a request to queue `P`'s engine, which its entries make",
    );
}

/// An engine family is refused before what it takes is read: a family of
/// engines is a queue family, whose entries are its requests (#539).
#[test]
fn an_engine_family_has_no_request() {
    let src = "
device gpu[2] { kv cap 64; }
engine llm[2] on gpu {
  tokens cap 8;
  scheduler { advance running; admit waiting while (running.preempted == 0); }
  worker { forward (1); }
}
server (n) { run llm[0] prefill (cost(llm, n)); }
";
    refused_with(
        src,
        "workload { arrive batch(1); turn { set n = 6; } send (n); }",
        "engine family `llm[2]`: a family of engines is a queue family",
    );
}

/// `request.hit` is `min(cachedin(P), reuse)`, and a connector's
/// `request.loaded` is read after it: a `reuse` that read either would read
/// the previous admission's (#555). A `prefix` segment reads no state.
#[test]
fn a_bound_of_the_hit_does_not_read_it() {
    let with = |clause: &str| {
        replaced(
            &program(),
            &[(
                "      cache (request.input + request.output);",
                &format!("      {clause}\n      cache (request.input + request.output);"),
            )],
        )
    };
    refused(
        &with("reuse (request.hit + 4)"),
        "a request's `reuse` reads `request.hit`, which the admission computes from it",
    );
    refused(
        &with("prefix (serial: request.hit)"),
        "a request's `prefix` segment reads the state",
    );
    refused(&with("reuse (request.loaded)"), "`request.loaded`");
    compiled(&with("reuse (request.known - 1)"));
}
