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

/// `text` with its declarations from `from` up to `to` replaced by `with`.
fn spliced(text: &str, from: &str, to: &str, with: &str) -> String {
    let i = text.find(from).unwrap_or_else(|| panic!("no {from:?}"));
    let j = i + text[i..].find(to).unwrap_or_else(|| panic!("no {to:?}"));
    format!("{}{with}{}", &text[..i], &text[j..])
}

/// `examples/multi-turn/vllm.sq`, an engine, and the step stage it is.
fn vllm_engine() -> (String, String) {
    let new = std::fs::read_to_string(root().join("examples/multi-turn/vllm.sq")).unwrap();
    let old = replaced(
        &spliced(
            &new,
            "  device gpu {",
            "  stage tool : delay;",
            "  pool kv { cap blocks * bs; block bs; evict lru; preempt lifo; }
  pool reqs { cap max_seqs; admit via vllm; }

  stage vllm : step {
    budget B;
    chunk residents + queued(reqs) > 1 ? chunk_cap : 0;
    cost c0 + max(omega + beta * (kv_decode + kv_prefill), tokens * a);
    memory kv;
  }
",
        ),
        &[(
            r#"args.number("chunk_cap", inf)"#,
            r#"args.number("chunk_cap", 0)"#,
        )],
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
    let tgi_old = spliced(
        &tgi,
        "  device gpu {",
        "  stage tool : delay;",
        "  pool kv { cap T; block 1; evict lru; preempt none; admit via tgi; }

  stage tgi : step {
    budget B;
    cost max(omega + beta * (kv_decode + kv_prefill), tokens * a);
    memory kv;
    state just = 0;
    iteration { serve; branch (just == 0 || residents == 0) { admit; } set just = admitted > 0; }
  }
",
    );
    assert_eq!(ir(&tgi_old, Some(&base), &ov), ir(&tgi, Some(&base), &ov));

    let sglang = std::fs::read_to_string(base.join("sglang.sq")).unwrap();
    let sglang_old = spliced(
        &sglang,
        "  device gpu {",
        "  stage tool : delay;",
        "  pool reqs { cap max_run; admit via sglang; }
  pool kv { cap tokens_cap; evict lru; preempt by (1 - decoding, position - prompt, -prompt) requeue tail; }

  stage sglang : step {
    budget B;
    cost max(omega + beta * (kv_decode + kv_prefill), tokens * a);
    memory kv;
    state ratio = r0;
    state backlog = 0;
    iteration {
      branch (residents == 0 && backlog == 0) { set ratio = r0; }
      serve only (!decoding);
      admit;
      branch (tokens == 0) {
        serve;
        branch (preempted > 0) {
          set ratio = max(r_min, min(1, retract_steps / M));
        } else {
          set ratio = max(r_min, ratio - r_decay);
        }
      }
      set backlog = queued(reqs) > 0;
    }
  }
",
    );
    assert_eq!(
        ir(&sglang_old, Some(&base), &ov),
        ir(&sglang, Some(&base), &ov)
    );
}

/// A small engine and the step stage it is, for the forms below.
const WORKLOAD: &str = "
workload { arrive batch(3); init { set prompt = 2; } }
server {
  hold reqs (cost(reqs, 1)), kv (cost(kv, prompt)) {
    run vllm prefill (cost(vllm, prompt)) growing kv;
    run vllm decode (cost(vllm, 2)) growing kv;
  }
}
";

fn engine(schedule: &str) -> String {
    format!(
        "device gpu {{ step_time (t) = 1; kv cap 100; }}
engine vllm on gpu {{
  reqs cap 8;
  tokens cap 8;
  schedule {{ {schedule} }}
  execute (step_time(batch.tokens));
}}
pool reqs on vllm {{ }}
pool kv on gpu {{ preempt lifo; }}
{WORKLOAD}"
    )
}

fn stage(options: &str) -> String {
    format!(
        "pool reqs {{ cap 8; admit via vllm; }}
pool kv {{ cap 100; preempt lifo; }}
stage vllm : step {{ budget 8; cost 1; memory kv; {options} }}
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

/// An engine names each value one way in all its clauses: `running.…` the
/// residents, `waiting.…` the queues, `batch.…` the batch, and none of them
/// reads the kernel's name for it (#411). The kernel's `decoders` and
/// `kv_decode` are the residents' before the batch is formed and the
/// batch's in `cost`, which a budget or an `only` may leave short, so an
/// engine reads `running.decoding` in `tokens cap` and `schedule` and
/// `batch.decoding` in `execute` (#416).
#[test]
fn an_engine_reads_its_lists_in_every_clause() {
    let ov = common::horizon(20.0);
    let ok = "advance running; admit waiting while (running.preempted == 0);";
    for (form, kernel) in [
        (
            (
                "tokens cap 8;",
                "tokens cap max(running.decoding, 8 - running.count) + running.kv_decode;",
            ),
            (
                "budget 8;",
                "budget max(decoders, 8 - residents) + kv_decode;",
            ),
        ),
        (
            ("tokens cap 8;", "tokens cap max(8 - waiting.count, 1);"),
            ("budget 8;", "budget max(8 - queued(reqs), 1);"),
        ),
        (
            (
                "execute (step_time(batch.tokens));",
                "execute (step_time(batch.tokens) + batch.decoding + batch.kv_decode + \
                 batch.prefilled + batch.attention + running.count + waiting.count);",
            ),
            (
                "cost 1;",
                "cost 1 + decoders + kv_decode + prefilled + attention + residents + queued(reqs);",
            ),
        ),
    ] {
        assert_eq!(
            ir(&replaced(&engine(ok), &[form]), None, &ov),
            ir(&replaced(&stage(""), &[kernel]), None, &ov),
            "{}",
            form.1
        );
    }
    for (from, to, why) in [
        (
            "tokens cap 8;",
            "tokens cap 8 - residents;",
            "`residents` is `running.count`",
        ),
        (
            "execute (step_time(batch.tokens));",
            "execute (step_time(batch.tokens) + max(residents, 1));",
            "`residents` is `running.count`",
        ),
        (
            "tokens cap 8;",
            "tokens cap max(decoders, 1);",
            "`decoders` is `running.decoding`",
        ),
        (
            "execute (step_time(batch.tokens));",
            "execute (step_time(batch.tokens) + running.decoding);",
            "counts the residents, in the batch or not, and the batch's is `batch.decoding`",
        ),
        (
            "execute (step_time(batch.tokens));",
            "execute (step_time(batch.tokens) + kv_decode);",
            "`kv_decode` is `batch.kv_decode` here",
        ),
        (
            "execute (step_time(batch.tokens));",
            "execute (step_time(batch.tokens) + tokens);",
            "`tokens` is `batch.tokens` here",
        ),
        (
            "tokens cap 8;",
            "tokens cap 8 - batch.tokens;",
            "`tokens cap` is read before the batch is formed",
        ),
        (
            "while (running.preempted == 0)",
            "while (batch.decoding == 0)",
            "`batch.decoding` is the formed batch's, read in `execute`",
        ),
        (
            "tokens cap 8;",
            "tokens cap 8 - batch.size;",
            "`batch` has `tokens`, `prefilled`, `decoding`",
        ),
        // a schedule's `only`, `by` and `each at most` are read at their own
        // moments, before the batch so far exists
        (
            "advance running;",
            "advance running only (batch.prefilled == 0);",
            "`only` is read before the batch is formed",
        ),
        (
            "advance running;",
            "advance running by (batch.tokens);",
            "`by` is read before the batch is formed",
        ),
        (
            "advance running;",
            "advance running only (running.preempted == 0);",
            "read in its `branch`, `while` and `set`, not in `only`",
        ),
        (
            "advance running;",
            "let c = batch.tokens > 0 ? 4 : 8; advance running each at most (c);",
            "`each at most` is read before the batch is formed",
        ),
        (
            "tokens cap 8;",
            "tokens cap 8 - attention;",
            "`attention` is `batch.attention`, and `tokens cap` is read before",
        ),
        (
            "tokens cap 8;",
            "tokens cap 8 - running.preempted;",
            "read in its `branch`, `while` and `set`, not in `tokens cap`",
        ),
        (
            "execute (step_time(batch.tokens));",
            "execute (step_time(batch.tokens) + waiting.admitted);",
            "read in its `branch`, `while` and `set`, not in `execute`",
        ),
        (
            "execute (step_time(batch.tokens));",
            "execute (step_time(batch.tokens) + preempted);",
            "`running.preempted` is what the iteration's schedule did",
        ),
        (
            "tokens cap 8;",
            "tokens cap max(tokens, 8);",
            "`tokens cap` is read before the batch is formed",
        ),
        (
            "while (running.preempted == 0)",
            "while (max(decoders, 1) == 1)",
            "`decoders` is `running.decoding`",
        ),
    ] {
        refused(&replaced(&engine(ok), &[(from, to)]), why);
    }
    // a schedule reads the batch it has formed so far
    assert_eq!(
        ir(
            &engine("advance running; branch (batch.tokens < 8) { admit waiting; }"),
            None,
            &ov
        ),
        ir(
            &stage("iteration { serve; branch (tokens < 8) { admit; } }"),
            None,
            &ov
        )
    );
    // and after an `only`, read for each resident, the statements read it again
    assert_eq!(
        ir(
            &engine(
                "advance running only (running.count > 0); branch (batch.tokens < 8) { admit \
                 waiting; }"
            ),
            None,
            &ov
        ),
        ir(
            &stage("iteration { serve only (residents > 0); branch (tokens < 8) { admit; } }"),
            None,
            &ov
        )
    );
    // the bug as found: a list value as a call argument, in a schedule
    assert_eq!(
        ir(
            &engine("advance running; admit waiting while (max(running.decoding, 1) == 1);"),
            None,
            &ov
        ),
        ir(
            &stage("iteration { serve; admit while (max(decoders, 1) == 1); }"),
            None,
            &ov
        )
    );
    // a step stage is no engine: it has no lists to name
    refused(
        &replaced(&stage(""), &[("budget 8;", "budget 8 - running.count;")]),
        "is a value of an engine",
    );
}

/// A predicate `advance running` and `admit waiting` share is named with a
/// `def`, read where each `only` stands, and the schedule is still vLLM's
/// procedure: `serve only (p)`, no iteration body (#412).
#[test]
fn a_def_names_the_predicate_advance_and_admit_share() {
    let ov = common::horizon(20.0);
    let p = "running.decoding > 0 ? decoding : !decoding";
    let named = format!(
        "def in_phase() {{ {p} }}\n{}",
        engine(
            "advance running only (in_phase()); admit waiting only (in_phase()) while \
             (running.preempted == 0);"
        )
    );
    // a `def` with an argument, and the other `only` written out
    let mixed = format!(
        "def in_phase(k) {{ running.decoding > k ? decoding : !decoding }}\n{}",
        engine(&format!(
            "advance running only (in_phase(0)); admit waiting only ({p}) while \
             (running.preempted == 0);"
        ))
    );
    let kernel = ir(
        &stage("serve only (decoders > 0 ? decoding : !decoding);"),
        None,
        &ov,
    );
    assert_eq!(ir(&named, None, &ov), kernel);
    assert_eq!(ir(&mixed, None, &ov), kernel);
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
            "pool reqs on vllm { }",
            "pool reqs on vllm { admit via vllm; }",
        ),
        "takes `admit` from it",
    );
    refused(
        &engine(ok).replace("pool kv on gpu { preempt lifo; }", "pool kv[2] on gpu { }"),
        "write no `[N]`",
    );
    refused(
        &engine(ok).replace("pool reqs on vllm { }", "pool slots on vllm { }"),
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
        &engine(ok).replace("pool reqs on vllm { }", ""),
        "is no pool",
    );
    // two pools on the engine's device
    refused(
        &engine(ok)
            .replace("kv cap 100;", "kv cap 100; enc cap 10;")
            .replace(
                "pool reqs on vllm { }",
                "pool reqs on vllm { } pool enc on gpu { }",
            ),
        "which is `vllm`'s KV",
    );
    // the engine's three parts
    refused(
        &engine(ok).replace("tokens cap 8;", ""),
        "needs `tokens cap B;`",
    );
    refused(
        &engine(ok).replace("execute (step_time(batch.tokens));", ""),
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
        &engine(
            "let phase = running.decoding > 0; advance running only (phase ? decoding : \
             !decoding); admit waiting only (phase ? decoding : !decoding) while \
             (running.preempted == 0);",
        ),
        "with `def phase() { … }`, read as `phase()`",
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
engine e1 on gpu {{ tokens cap 4; schedule {{ advance running; }} execute (t(batch.tokens)); }}
engine e2 on gpu {{ tokens cap 4; schedule {{ advance running; }} execute (t(batch.tokens)); }}
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
            "engine vllm on gpu",
            "def step_time(x) { x }\nengine vllm on gpu",
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
    // one engine per device, as many as its devices
    refused(
        &engine(ok).replace("engine vllm on gpu", "engine vllm[2] on gpu"),
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
    run vllm[j] prefill (cost(vllm, prompt)) growing kv[j];
  }
}
";
    let new = format!(
        "device gpu[2] {{ step_time (t) = 1; kv cap 100; }}
engine vllm[2] on gpu {{
  tokens cap 8;
  schedule {{ advance running; admit waiting while (running.preempted == 0); }}
  execute (step_time(batch.tokens));
}}
pool kv on gpu {{ }}
{work}"
    );
    let old = format!(
        "pool kv[2] {{ cap 100; admit via vllm; }}
stage vllm[2] : step {{ budget 8; cost 1; memory kv; }}
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
    // nor its `tokens cap`
    let tokens_cap = new
        .lines()
        .find(|l| l.trim_start().starts_with("tokens cap"))
        .expect("the engine's tokens cap")
        .trim()
        .to_string();
    refused(
        &new.replace(&tokens_cap, "tokens cap 8 + waiting.count;"),
        "which no member's engine may",
    );
}

/// Inside a `queue`, `device gpu` is the member's and `engine on gpu` is
/// the queue's stage: `examples/pd-disaggregation/llmd_nixl_pull.sq`, which
/// writes both pods so, has the IR of its pods written as `serve step`. The decoder's holds name `kv` first,
/// so its device pool is admitted by its engine; the prefiller's are not.
#[test]
fn a_queue_holds_its_engine() {
    let base = root().join("examples/pd-disaggregation");
    let new = std::fs::read_to_string(base.join("llmd_nixl_pull.sq")).unwrap();
    let mut old = new.clone();
    for (q, cap, blocks) in [("P", "max_seqsP", "blocksP"), ("D", "max_seqsD", "blocksD")] {
        let via = if q == "D" { " admit via D;" } else { "" };
        old = replaced(
            &old,
            &[(
                &format!(
                    "    device gpu {{ compute (t) = t * a; hbm (k) = omega + beta * k; kv cap {blocks} * bs; }}
    engine on gpu {{
      reqs cap {cap};
      tokens cap B;
      schedule {{ advance running; admit waiting while (running.preempted == 0); }}
      execute (c0 + max(hbm(batch.kv_decode + batch.kv_prefill), compute(batch.tokens)));
    }}
    pool reqs on {q} {{ queue fifo; }}
    pool kv on gpu {{ block bs; evict lru; preempt lifo; }}
"
                ),
                &format!(
                    "    pool reqs {{ cap {cap}; admit via {q}; }}
    pool kv {{ cap {blocks} * bs; block bs; evict lru; preempt lifo;{via} }}
    serve step {{
      budget B;
      cost c0 + max(omega + beta * (kv_decode + kv_prefill), tokens * a);
      memory kv;
    }}
"
                ),
            )],
        );
    }
    let ov = common::horizon(100.0);
    assert_eq!(ir(&old, Some(&base), &ov), ir(&new, Some(&base), &ov));
}

#[test]
fn a_queues_engine_is_its_stage() {
    let pod = |items: &str| {
        format!(
            "queue E : prefill {{
  {items}
  prefill (prompt) {{
    hold kv (cost(kv, prompt)) {{ run E prefill (cost(E, prompt)) growing kv; }}
  }}
}}
workload {{ arrive batch(1); init {{ set prompt = 2; }} }}
server {{ E.prefill (prompt); }}
"
        )
    };
    let device = "device gpu { t1 (x) = 1; kv cap 10; }";
    let engine = "engine on gpu { tokens cap 4; schedule { advance running; admit waiting while (running.preempted == 0); } execute (t1(batch.tokens)); }";
    let ov = common::horizon(10.0);
    ir(
        &pod(&format!("{device} {engine} pool kv on gpu {{ }}")),
        None,
        &ov,
    );
    refused(
        &pod(&format!(
            "{} {engine} pool kv on gpu {{ }}",
            device.replace("gpu", "gpu[2]")
        )),
        "a queue's device is the member's",
    );
    refused(
        &pod(&format!(
            "{device} {engine} serve fifo; pool kv on gpu {{ }}"
        )),
        "is one stage",
    );
    refused(
        &pod(&format!("{engine} {device} pool kv on gpu {{ }}")),
        "no device `gpu` in queue `E`",
    );
    refused(
        &pod(&format!("{device} {engine} {engine} pool kv on gpu {{ }}")),
        "has one stage",
    );
    // a queue's pool is on its own device or engine, never one outside it
    let other = format!(
        "queue F[3] {{ device g {{ t1 (x) = 1; }} {} }}\n",
        engine.replace("gpu", "g")
    );
    refused(
        &format!(
            "{other}{}",
            pod(&format!(
                "{device} {engine} pool kv on gpu {{ }} pool r on F {{ }}"
            ))
        ),
        "`F` is not a device or the engine of queue `E`",
    );
    refused(
        &format!(
            "device gtop {{ t1 (x) = 1; }}\n{}",
            pod(&format!("{device} {engine} pool kv on gtop {{ }}"))
        ),
        "`gtop` is not a device or the engine of queue `E`",
    );
    refused(
        &format!(
            "{}pool reqs on E {{ }}\n",
            pod(&format!("{device} {engine} pool kv on gpu {{ }}"))
        ),
        "declare `pool reqs on E` in queue `E`",
    );
    // a queue and a top-level device of one name: refused in either order
    let top = "device E { t1 (x) = 1; z cap 2; } pool z on E { }\n";
    let queue = pod(&format!("{device} {engine} pool kv on gpu {{ }}"));
    let (decls, rest) = queue.split_at(queue.find("workload").unwrap());
    refused(&format!("{top}{decls}{rest}"), "`E` is declared twice");
    refused(&format!("{decls}{top}{rest}"), "`E` is declared twice");
    // a gateway has no stage, and is named all the same
    let gateway = "queue E : gateway { route () { } }\n";
    refused(&format!("{top}{gateway}"), "`E` is declared twice");
    refused(&format!("{gateway}{top}"), "`E` is declared twice");
    // the names a declaration outside a queue may not take
    refused(
        &pod(&format!("{device} {engine} pool kv on E {{ }}").replace("gpu", "E")),
        "a queue's device needs another name",
    );
    refused(
        &pod(&format!("{device} {engine} pool kv on gpu {{ }}"))
            .replace("queue E", "queue engine")
            .replace("run E", "run engine")
            .replace("cost(E,", "cost(engine,")
            .replace("E.prefill", "engine.prefill"),
        "a queue's engine is named after the queue",
    );
    refused(
        &pod(&format!("{device} {engine} pool kv on gpu {{ }}").replace("gpu", "schedule")),
        "`schedule` is a word of the language",
    );
    refused(
        &pod(&format!("{device} {engine} pool kv on kv {{ }}").replace("gpu", "kv")),
        "`kv` is declared twice in queue `E`",
    );
    // an error names a queue's device as the program does
    refused(
        &pod(&format!("{device} {engine} pool kv on gpu {{ }}")
            .replace(" kv cap 10;", " kv cap 10; enc cap 5;")),
        "`enc` of `gpu` of queue `E` is no pool: declare `pool enc on gpu { … }` in queue `E`",
    );
    // #403's checks hold for a queue's engine
    refused(
        &pod(&format!("{device} {engine} pool kv on gpu {{ }}")).replace(
            "advance running; admit waiting while (running.preempted == 0);",
            "advance running each at most (0);",
        ),
        "`each at most (0)`",
    );
    refused(
        &pod(&format!("{device} {engine} pool kv on gpu {{ }}"))
            .replace("queue E :", "queue E[2] :")
            .replace("E.prefill", "E[0].prefill")
            .replace(
                "while (running.preempted == 0)",
                "while (waiting.count > 0)",
            ),
        "its `waiting.count` would read every member's",
    );
}
