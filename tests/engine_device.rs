//! Engines on devices (`docs/design/engine-device.md`): `device`, `engine …
//! on`, `pool … on`, `schedule` and `execute` are parse-time sugar, so a
//! program written with them has the IR of the one written with a step
//! stage, and every rule the design refuses does not link.

mod common;

use std::path::Path;

use serq::{Overrides, compile_source_at};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn ir(src: &str, base: Option<&Path>, ov: &Overrides) -> String {
    let p = compile_source_at(&common::main_source(src), base, ov)
        .unwrap_or_else(|e| panic!("{e}\n{src}"));
    serde_json::to_string_pretty(&p).unwrap()
}

fn error(src: &str) -> String {
    match compile_source_at(&common::main_source(src), None, &common::horizon(10.0)) {
        Ok(_) => panic!("links:\n{src}"),
        Err(e) => e,
    }
}

fn refused(src: &str, fragment: &str) {
    let e = error(src);
    assert!(e.contains(fragment), "missing {fragment:?}:\n{e}");
}

fn replaced(text: &str, pairs: &[(&str, &str)]) -> String {
    let mut t = text.to_string();
    for (a, b) in pairs {
        assert_eq!(t.matches(a).count(), 1, "{a}");
        t = t.replace(a, b);
    }
    t
}

/// `examples/multi-turn/vllm.sq`'s step stage, written as an engine.
fn vllm_engine() -> (String, String) {
    let old = std::fs::read_to_string(root().join("examples/multi-turn/vllm.sq")).unwrap();
    let new = replaced(
        &old,
        &[
            (
                r#"let chunk_cap = args.number("chunk_cap", 0);"#,
                r#"let chunk_cap = args.number("chunk_cap", inf);"#,
            ),
            (
                "  pool kv { cap blocks * bs; block bs; evict lru; preempt lifo; }
  pool reqs { cap max_seqs; admit via engine; }   // the engine's step admits the waiting, FCFS

  stage engine : step {
    budget B;
    chunk long_prefill(reqs, chunk_cap);
    cost c0 + max(omega + beta * (kv_decode + kv_prefill), tokens * a);
    memory kv;
  }
",
                "  device gpu {
    compute (t) = t * a;
    hbm (k) = omega + beta * k;
    kv cap blocks * bs;
  }
  engine engine on gpu {
    reqs cap max_seqs;
    tokens cap B;
    schedule {
      let threshold = running.count + waiting.count > 1 ? chunk_cap : inf;
      advance running each at most (threshold);
      admit waiting while (running.preempted == 0) each at most (threshold);
    }
    execute (c0 + max(hbm(kv_decode + kv_prefill), compute(tokens)));
  }
  pool kv on gpu { block bs; evict lru; preempt lifo; }
  pool reqs on engine { queue fifo; }
",
            ),
        ],
    );
    (old, new)
}

#[test]
fn vllm_as_an_engine_has_the_step_stages_ir() {
    let (old, new) = vllm_engine();
    let base = root().join("examples/multi-turn");
    let ov = common::horizon(100.0);
    // no cap (0 in the stage, `inf` in the engine: 0 in the kernel either
    // way), and a cap of 512
    assert_eq!(ir(&old, Some(&base), &ov), ir(&new, Some(&base), &ov));
    let old512 = old.replace(
        r#"args.number("chunk_cap", 0)"#,
        r#"args.number("chunk_cap", 512)"#,
    );
    let new512 = new.replace(
        r#"args.number("chunk_cap", inf)"#,
        r#"args.number("chunk_cap", 512)"#,
    );
    assert_eq!(ir(&old512, Some(&base), &ov), ir(&new512, Some(&base), &ov));
}

/// SGLang's body and TGI's, which the kernel keeps as bodies. TGI's holds
/// name `kv` first, so its device pool is admitted by the engine; SGLang's
/// `waiting.count` is the one queue its engine admits.
#[test]
fn sglang_and_tgi_as_engines_have_their_irs() {
    let base = root().join("examples/engines");
    let ov = common::horizon(100.0);
    let tgi = std::fs::read_to_string(base.join("tgi.sq")).unwrap();
    let tgi_new = replaced(
        &tgi,
        &[(
            "  pool kv { cap T; block 1; evict lru; preempt none; admit via engine; }

  stage engine : step {
    budget B;
    cost max(omega + beta * (kv_decode + kv_prefill), tokens * a);
    memory kv;
    state just = 0;                // the last forward admitted
    // with no running batch there is no decode forward, and the queue is read
    iteration { serve; branch (just == 0 || residents == 0) { admit; } set just = admitted > 0; }
  }
",
            "  device gpu { compute (t) = t * a; hbm (k) = omega + beta * k; kv cap T; }
  engine engine on gpu {
    tokens cap B;
    state just = 0;
    schedule {
      advance running;
      branch (just == 0 || running.count == 0) { admit waiting; }
      set just = waiting.admitted > 0;
    }
    execute (max(hbm(kv_decode + kv_prefill), compute(tokens)));
  }
  pool kv on gpu { block 1; evict lru; preempt none; }
",
        )],
    );
    assert_eq!(ir(&tgi, Some(&base), &ov), ir(&tgi_new, Some(&base), &ov));

    let sglang = std::fs::read_to_string(base.join("sglang.sq")).unwrap();
    let start = sglang
        .find("  pool reqs { cap max_run; admit via engine; }")
        .unwrap();
    let end = sglang.find("  stage tool : delay;").unwrap();
    let block = &sglang[start..end];
    let body = &block[block.find("    iteration {").unwrap()..block.rfind("  }").unwrap()];
    let body = replaced(
        body,
        &[
            ("    iteration {", "    schedule {"),
            (
                "serve only (!decoding);",
                "advance running only (!decoding);",
            ),
            ("      admit;", "      admit waiting;"),
            ("        serve;", "        advance running;"),
            ("residents == 0", "running.count == 0"),
            ("preempted > 0", "running.preempted > 0"),
            ("queued(reqs) > 0", "waiting.count > 0"),
        ],
    );
    let engine = format!(
        "  device gpu {{ compute (t) = t * a; hbm (k) = omega + beta * k; kv cap tokens_cap; }}
  engine engine on gpu {{
    reqs cap max_run;
    tokens cap B;
    state ratio = r0;
    state backlog = 0;
{body}    execute (max(hbm(kv_decode + kv_prefill), compute(tokens)));
  }}
  pool reqs on engine {{ queue fifo; }}
  pool kv on gpu {{ evict lru; preempt by (1 - decoding, position - prompt, -prompt) requeue tail; }}
"
    );
    let sglang_new = format!("{}{engine}{}", &sglang[..start], &sglang[end..]);
    assert_eq!(
        ir(&sglang, Some(&base), &ov),
        ir(&sglang_new, Some(&base), &ov)
    );
}

/// A small engine and the step stage it is, for the forms below.
const WORKLOAD: &str = "
workload { arrive batch(3); init { set prompt = 2; } }
server {
  hold reqs (cost(reqs, 1)), kv (cost(kv, prompt)) {
    run engine prefill (cost(engine, prompt)) growing kv;
    run engine decode (cost(engine, 2)) growing kv;
  }
}
";

fn engine(schedule: &str) -> String {
    format!(
        "device gpu {{ step_time (t) = 1; kv cap 100; }}
engine engine on gpu {{
  reqs cap 8;
  tokens cap 8;
  schedule {{ {schedule} }}
  execute (step_time(tokens));
}}
pool reqs on engine {{ }}
pool kv on gpu {{ preempt lifo; }}
{WORKLOAD}"
    )
}

fn stage(options: &str) -> String {
    format!(
        "pool reqs {{ cap 8; admit via engine; }}
pool kv {{ cap 100; preempt lifo; }}
stage engine : step {{ budget 8; cost 1; memory kv; {options} }}
{WORKLOAD}"
    )
}

#[test]
fn the_stage_forms_are_schedules() {
    let ov = common::horizon(20.0);
    for (schedule, options) in [
        (
            "advance running; admit waiting while (running.preempted == 0);",
            "",
        ),
        (
            "advance running; admit waiting while (!running.preempted);",
            "",
        ),
        (
            "advance running decode first; admit waiting while (running.preempted == 0);",
            "serve decode first;",
        ),
        (
            "advance running by (remaining); admit waiting while (running.preempted == 0);",
            "serve by (remaining);",
        ),
        (
            "advance running only (decoding); admit waiting only (decoding) while (running.preempted == 0);",
            "serve only (decoding);",
        ),
        (
            "exclusive prefill; admit waiting while (running.preempted == 0);",
            "serve exclusive prefill;",
        ),
        (
            "advance running each at most (4); admit waiting while (running.preempted == 0) each at most (4);",
            "chunk 4;",
        ),
        (
            "advance running; branch (running.count == 0) { admit waiting; }",
            "iteration { serve; branch (residents == 0) { admit; } }",
        ),
        (
            "admit waiting; advance running;",
            "iteration { admit; serve; }",
        ),
    ] {
        assert_eq!(
            ir(&engine(schedule), None, &ov),
            ir(&stage(options), None, &ov),
            "{schedule}"
        );
    }
}

#[test]
fn the_design_refuses_what_it_says() {
    let ok = "advance running; admit waiting while (running.preempted == 0);";
    // a device's time resource is read in an engine's `execute` only
    refused(
        &engine(ok).replace("tokens cap 8;", "tokens cap step_time(1);"),
        "read in an engine's `execute`",
    );
    // a pool on a device or an engine takes its cap and its admitter from it
    refused(
        &engine(ok).replace(
            "pool kv on gpu { preempt lifo; }",
            "pool kv on gpu { cap 5; }",
        ),
        "takes `cap` from it",
    );
    refused(
        &engine(ok).replace(
            "pool reqs on engine { }",
            "pool reqs on engine { admit via engine; }",
        ),
        "takes `admit` from it",
    );
    refused(
        &engine(ok).replace("pool kv on gpu { preempt lifo; }", "pool kv[2] on gpu { }"),
        "write no `[N]`",
    );
    refused(
        &engine(ok).replace("pool reqs on engine { }", "pool slots on engine { }"),
        "has no capacity `slots`",
    );
    refused(
        &engine(ok).replace(
            "pool kv on gpu { preempt lifo; }",
            "pool step_time on gpu { }",
        ),
        "a time resource",
    );
    // a capacity no pool declares
    refused(
        &engine(ok).replace("pool reqs on engine { }", ""),
        "is no pool",
    );
    // two pools on the engine's device
    refused(
        &engine(ok)
            .replace("kv cap 100;", "kv cap 100; enc cap 10;")
            .replace(
                "pool reqs on engine { }",
                "pool reqs on engine { } pool enc on gpu { }",
            ),
        "which is `engine`'s KV",
    );
    // the engine's three parts
    refused(
        &engine(ok).replace("tokens cap 8;", ""),
        "needs `tokens cap B;`",
    );
    refused(
        &engine(ok).replace("execute (step_time(tokens));", ""),
        "needs `execute (T);`",
    );
    refused(
        &engine(ok).replace("tokens cap 8;", "tokens cap 0;"),
        "`tokens cap` at or below 0",
    );
    refused(
        &engine(ok).replace("tokens cap 8;", "budget 8;"),
        "`tokens cap B;`",
    );
    refused(
        &engine(ok).replace("reqs cap 8;", "reqs cap 8; memory kv;"),
        "the pool on its device",
    );
    // a schedule
    refused(
        &engine("advance running; let c = 4; admit waiting;"),
        "stands before its statements",
    );
    refused(
        &engine("let c = 4; advance running; branch (c > 1) { admit waiting; }"),
        "read only in `each at most`",
    );
    refused(
        &engine("advance running each at most (4); admit waiting each at most (5);"),
        "an iteration has one cap per run",
    );
    refused(
        &engine("advance running each at most (4); admit waiting;"),
        "an iteration has one cap per run",
    );
    refused(
        &engine("advance running each at most (min(inf, 4));"),
        "`inf` inside an `each at most`'s arithmetic",
    );
    refused(
        &engine("advance running each at most (0);"),
        "would be given nothing",
    );
    refused(
        &engine("advance running each at most (-4);"),
        "would be given nothing",
    );
    // a condition known once linked chooses its branch: vLLM's 0 for none
    assert_eq!(
        ir(
            &engine(
                "let c = 0; let t = c > 0 ? c : inf; advance running each at most (t); \
                 admit waiting while (running.preempted == 0) each at most (t);"
            ),
            None,
            &common::horizon(20.0)
        ),
        ir(&stage(""), None, &common::horizon(20.0))
    );
    refused(&engine("advance running each at most (foo);"), "unknown");
    refused(
        &engine("advance running each at most (2 - 2);"),
        "would be given nothing",
    );
    refused(
        &engine("let t = running.count > 1 ? -1 : inf; advance running each at most (t);"),
        "would be given nothing",
    );
    refused(
        &engine("advance running each at most (running.count);"),
        "chooses among constants",
    );
    // a `let` reads the ones above it
    let ov = common::horizon(20.0);
    assert_eq!(
        ir(
            &engine(
                "let a = 4; let b = a + 1; advance running each at most (b); \
                 admit waiting while (running.preempted == 0) each at most (b);"
            ),
            None,
            &ov
        ),
        ir(&stage("chunk 5;"), None, &ov)
    );
    // one device runs one engine, with pools on it or none
    refused(
        &format!(
            "device gpu {{ t (x) = 1; }}
engine e1 on gpu {{ tokens cap 4; schedule {{ advance running; }} execute (t(tokens)); }}
engine e2 on gpu {{ tokens cap 4; schedule {{ advance running; }} execute (t(tokens)); }}
{WORKLOAD}"
        ),
        "one device runs one engine",
    );
    refused(
        &engine(ok).replace(
            "pool kv on gpu { preempt lifo; }",
            "pool kv on gpu { } pool kv on gpu { }",
        ),
        "declared as a pool twice",
    );
    refused(
        &engine(ok).replacen(
            "engine engine on gpu",
            "def step_time(x) { x }\nengine engine on gpu",
            1,
        ),
        "a device's time resource",
    );
    refused(
        &format!("queue waiting : link {{ serve fifo; }} {}", engine(ok)),
        "a queue needs another name",
    );
    refused(
        &engine("advance running; branch (residents == 0) { admit waiting; }"),
        "`running.count`",
    );
    refused(
        &engine("advance running; admit waiting while (!preempted);"),
        "`running.preempted`",
    );
    refused(&engine("serve; admit waiting;"), "`advance running`");
    refused(&engine("advance running; admit;"), "`admit waiting;`");
    refused(
        &engine("branch (running.size > 0) { advance running; }"),
        "`running` has `count`",
    );
    refused(
        &engine("exclusive prefill; branch (running.count == 0) { admit waiting; }"),
        "takes back decodes already chosen",
    );
    refused(
        &engine(ok).replace("tokens cap 8;", "tokens cap 8 + running.count;"),
        "read in its `schedule`",
    );
    // one engine per device, as many as its devices
    refused(
        &engine(ok).replace("engine engine on gpu", "engine engine[2] on gpu"),
        "an engine is one per device",
    );
    refused(
        &engine(ok).replace("on gpu {\n  reqs", "on cpu {\n  reqs"),
        "no device `cpu`",
    );
}

/// A family of devices: `pool kv on gpu` is `kv[N]`, an engine `E[N]`
/// reads `kv[i]` from `E[i]`, as `stage E[N] : step { memory kv; }` beside
/// `pool kv[N]` does.
#[test]
fn a_family_follows_its_device() {
    let ov = common::horizon(20.0);
    let work = "
workload { arrive batch(4); init { set prompt = 2; } }
server {
  choose j in 2 by (holders(kv[j]));
  hold kv[j] (cost(kv, prompt)) {
    run engine[j] prefill (cost(engine, prompt)) growing kv[j];
  }
}
";
    let new = format!(
        "device gpu[2] {{ step_time (t) = 1; kv cap 100; }}
engine engine[2] on gpu {{
  tokens cap 8;
  schedule {{ advance running; admit waiting while (running.preempted == 0); }}
  execute (step_time(tokens));
}}
pool kv on gpu {{ }}
{work}"
    );
    let old = format!(
        "pool kv[2] {{ cap 100; admit via engine; }}
stage engine[2] : step {{ budget 8; cost 1; memory kv; }}
{work}"
    );
    assert_eq!(ir(&old, None, &ov), ir(&new, None, &ov));
    // a member's schedule cannot count every member's queues
    refused(
        &new.replace(
            "schedule { advance running; admit waiting while (running.preempted == 0); }",
            "schedule { let c = waiting.count > 0 ? 2 : inf; advance running each at most (c); \
             admit waiting while (running.preempted == 0) each at most (c); }",
        ),
        "is a family, and its `waiting.count`",
    );
}
