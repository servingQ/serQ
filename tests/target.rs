//! `serq target`: a program on vLLM's architecture is synthesised to the
//! scheduler configuration that runs it. The oracle closes the loop: each
//! scenario's program, synthesised, gives the configuration the oracle
//! drove the real scheduler with (`tools/oracle/<name>.json`), and
//! `tests/vllm_oracle.rs` checks the scheduler so configured decides as the
//! program does.

mod common;

use serde_json::Value;

fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn target(path: &str) -> Result<Value, String> {
    let p = serq::load(&root().join(path), &common::horizon(10.0))?;
    serq::target::vllm(&p)
}

#[test]
fn each_oracle_program_synthesises_to_the_configuration_its_scenario_ran() {
    for name in ["chunked", "hol", "longchunk", "mixed", "preempt", "seqcap"] {
        let sc: Value = serde_json::from_str(
            &std::fs::read_to_string(root().join(format!("tools/oracle/{name}.json"))).unwrap(),
        )
        .unwrap();
        let c = target(&format!("tools/oracle/{name}.ir.json")).unwrap()["config"].clone();
        let want = [
            ("max_num_batched_tokens", sc["budget"].clone()),
            ("max_num_seqs", sc["max_seqs"].clone()),
            ("block_size", sc["block_size"].clone()),
            ("num_gpu_blocks", sc["num_blocks"].clone()),
            (
                "long_prefill_token_threshold",
                sc.get("chunk").cloned().unwrap_or(0.into()),
            ),
            (
                "enable_prefix_caching",
                sc.get("prefix_caching").cloned().unwrap_or(false.into()),
            ),
        ];
        for (key, v) in want {
            assert_eq!(c[key], v, "{name}: {key}");
        }
    }
}

/// A cache clause on the KV pool is vLLM's prefix cache.
#[test]
fn the_vllm_programs_are_on_vllms_architecture() {
    for path in [
        "examples/multi-turn/vllm.sq",
        "examples/multi-turn/vllm_chat.sq",
        "examples/replay/vllm_replay.sq",
    ] {
        let t = target(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(t["config"]["enable_prefix_caching"], true, "{path}");
    }
    let t = target("examples/multi-turn/vllm.sq").unwrap();
    assert_eq!(t["config"]["num_gpu_blocks"], 10001);
    assert_eq!(t["config"]["max_num_batched_tokens"], 8192);
}

/// A policy vLLM's scheduler does not have is refused with the construct.
#[test]
fn another_policy_is_refused_with_its_construct() {
    for (path, why) in [
        ("examples/vendors/rbln.sq", "schedules `exclusive prefill`"),
        (
            "examples/single-turn/fastertransformer.sq",
            "has its own schedule",
        ),
        ("examples/single-turn/mg1.sq", "vLLM is one engine"),
        (
            "examples/engines/tensorrt_llm.sq",
            "does not preempt as vLLM does",
        ),
        (
            "examples/pd-disaggregation/vllm_nixl_push.sq",
            "vLLM is one engine",
        ),
    ] {
        let e = target(path).unwrap_err();
        assert!(
            e.contains("not on vLLM's architecture") && e.contains(why),
            "{path}: {e}"
        );
    }
    let src = "device gpu { kv cap 160; } pool kv on gpu { block 16; evict lru; preempt lifo; } pool reqs { cap 4; }
        engine llm on gpu {
          tokens cap 64;
          schedule { advance running by (remaining); admit waiting while (running.preempted == 0); }
          execute (1);
        }
        workload { arrive batch(1); init { set n = 8; }
          session { turn; end;
          }
        }
        server { hold reqs (cost(reqs, 1)), kv (cost(kv, n)) { run llm prefill (cost(llm, n)) growing kv; }
        } ";
    let p = serq::compile_source(&common::main_source(src), &common::horizon(10.0)).unwrap();
    let e = serq::target::vllm(&p).unwrap_err();
    assert!(
        e.contains("advances running by `remaining`")
            && e.contains("observes `decoding` and `admission`"),
        "{e}"
    );
}

/// Keys vLLM's scheduler can observe go to the programmable scheduler,
/// `tools/serq_vllm.py`, as the IR writes them.
#[test]
fn observable_serve_keys_name_the_programmable_scheduler() {
    let src = "device gpu { kv cap 160; } pool kv on gpu { block 16; evict lru; preempt lifo; } pool reqs { cap 4; }
        engine llm on gpu {
          tokens cap 64;
          schedule { advance running by (decoding ? 0 : 1, -admission); admit waiting while (running.preempted == 0); }
          execute (1);
        }
        workload { arrive batch(1); init { set n = 8; }
          session { turn; end;
          }
        }
        server { hold reqs (cost(reqs, 1)), kv (cost(kv, n)) { run llm prefill (cost(llm, n)) growing kv; }
        } ";
    let p = serq::compile_source(&common::main_source(src), &common::horizon(10.0)).unwrap();
    let t = serq::target::vllm(&p).unwrap();
    assert_eq!(t["config"]["scheduler_cls"], "serq_vllm.SerqScheduler");
    assert_eq!(
        t["serve_by"][1],
        serde_json::json!({"Unary": ["Neg", {"Ctx": "Admission"}]})
    );
    let fcfs = serq::target::vllm(
        &serq::load(
            &root().join("examples/multi-turn/vllm.sq"),
            &common::horizon(10.0),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(fcfs["config"].get("scheduler_cls").is_none() && fcfs.get("serve_by").is_none());
}

/// vLLM lifts the chunk cap for a request alone (scheduler.py:606-616), so a
/// constant cap is not vLLM's: the target takes the cap that holds only with
/// another request eligible, or none.
#[test]
fn a_constant_chunk_cap_is_refused_and_the_rule_taken() {
    let src = std::fs::read_to_string(root().join("examples/multi-turn/vllm.sq")).unwrap();
    let dir = root().join("examples/multi-turn");
    let rule = "let threshold = running.count + waiting.count > 1 ? chunk_cap : inf;";
    assert!(src.contains(rule), "the program's chunk moved");
    let p = serq::compile_source_at(
        &common::main_source(&src),
        Some(&dir),
        &common::horizon(10.0),
    )
    .unwrap();
    assert_eq!(
        serq::target::vllm(&p).unwrap()["config"]["long_prefill_token_threshold"],
        0
    );
    let mut ov = common::horizon(10.0);
    ov.set("chunk_cap", "512").unwrap();
    let p = serq::compile_source_at(&common::main_source(&src), Some(&dir), &ov).unwrap();
    assert_eq!(
        serq::target::vllm(&p).unwrap()["config"]["long_prefill_token_threshold"],
        512
    );
    let p = serq::compile_source_at(
        &common::main_source(&src.replace(rule, "let threshold = 512;")),
        Some(&dir),
        &common::horizon(10.0),
    )
    .unwrap();
    let e = serq::target::vllm(&p).unwrap_err();
    assert!(
        e.contains("the `each at most` is `512`")
            && e.contains("running.count + waiting.count > 1"),
        "{e}"
    );
}

/// vLLM's adaptive threshold (config/scheduler.py:87-91) floors the cap at
/// the budget's share, `max(c, input_budget // num_eligible_reqs)`
/// (scheduler.py:617-622): the target takes that cap, with `B` the `tokens
/// cap`, and refuses another share with the reason (#451).
#[test]
fn the_adaptive_chunk_cap_is_taken_and_another_share_refused() {
    let src = std::fs::read_to_string(root().join("examples/multi-turn/vllm.sq")).unwrap();
    let dir = root().join("examples/multi-turn");
    let rule = "let threshold = running.count + waiting.count > 1 ? chunk_cap : inf;";
    assert!(src.contains(rule), "the program's chunk moved");
    let target_as = |n: &str, threshold: &str| {
        let src = src.replace(rule, &format!("let n = {n}; let threshold = {threshold};"));
        let p = serq::compile_source_at(
            &common::main_source(&src),
            Some(&dir),
            &common::horizon(10.0),
        )
        .unwrap();
        serq::target::vllm(&p)
    };
    let target = |cap: &str| {
        target_as(
            "running.count + waiting.count",
            &format!("n > 1 ? {cap} : inf"),
        )
    };
    for cap in ["max(1000, floor(B / n))", "max(floor(B / n), 1000)"] {
        let c = target(cap).unwrap_or_else(|e| panic!("{cap}: {e}"))["config"].clone();
        assert_eq!(c["long_prefill_token_threshold"], 1000, "{cap}");
        assert_eq!(c["long_prefill_token_threshold_adaptive"], true, "{cap}");
    }
    // the count either way round
    let c = target_as(
        "waiting.count + running.count",
        "n > 1 ? max(1000, floor(B / n)) : inf",
    )
    .unwrap()["config"]
        .clone();
    assert_eq!(c["long_prefill_token_threshold_adaptive"], true);
    // the share without the condition holds for a request alone, where
    // vLLM lifts the cap
    let e = target_as("running.count + waiting.count", "max(1000, floor(B / n))").unwrap_err();
    assert!(
        e.contains("write it under `n > 1 ? … : inf`") && e.contains("scheduler.py:609-616"),
        "{e}"
    );
    // a constant cap is not adaptive, and says nothing of it
    let c = target("1000").unwrap()["config"].clone();
    assert_eq!(c["long_prefill_token_threshold"], 1000);
    assert!(c.get("long_prefill_token_threshold_adaptive").is_none());
    // another share: of a budget not the engine's, of another count, or
    // without the floor
    for cap in [
        "max(1000, floor(4096 / n))",
        "max(1000, floor(B / (n + 1)))",
        "max(1000, B / n)",
        "max(1000, floor(B / n) + 1)",
    ] {
        let e = target(cap).unwrap_err();
        assert!(
            e.contains("the cap computes a share other than vLLM's")
                && e.contains("scheduler.py:617-622"),
            "{cap}: {e}"
        );
    }
}

/// What vLLM's scheduler does not have since the iteration became a body:
/// a body of its own (and with it a register), a held reservation, a
/// request's legs.
/// vLLM's schedule written out is vLLM: it lowers to no body.
#[test]
fn the_newer_constructs_are_refused_and_vllms_schedule_taken() {
    let base = "device gpu { kv cap 160; }
        engine llm on gpu { reqs cap 4; tokens cap 64; STATE schedule { SCHEDULE } execute (1); }
        pool kv on gpu { block 16; evict lru; preempt lifo; }
        pool reqs on llm { RESERVE }
        workload { arrive batch(1); init { set n = 8; }
          session { turn; HOLD end;
          }
        }
        server {
        } ";
    let hold = "hold reqs (cost(reqs, 1)), kv (cost(kv, n)) { run llm prefill (cost(llm, n)) growing kv; }";
    let vllm = "advance running; admit waiting while (running.preempted == 0);";
    let compile = |reserve: &str, state: &str, schedule: &str, h: &str| {
        let src = base
            .replace("RESERVE", reserve)
            .replace("STATE", state)
            .replace("SCHEDULE", schedule)
            .replace("HOLD", h);
        serq::compile_source(&common::main_source(&src), &common::horizon(10.0)).unwrap()
    };
    serq::target::vllm(&compile("", "", vllm, hold)).unwrap();
    let legs = "fork { hold reqs (cost(reqs, 1)), kv (cost(kv, n)) { run llm prefill (cost(llm, n)) growing kv; } } join;";
    for (reserve, state, schedule, h, why) in [
        (
            "",
            "",
            "advance running; admit waiting;",
            hold,
            "has its own schedule",
        ),
        // a register is set by a body, so a program with one has its own
        (
            "",
            "state k = 0;",
            "advance running; admit waiting while (running.preempted == 0); set k = k + 1;",
            hold,
            "has its own schedule",
        ),
        ("reserve held;", "", vllm, hold, "holds its reservations"),
        ("", "", vllm, legs, "fork"),
    ] {
        let e = serq::target::vllm(&compile(reserve, state, schedule, h)).unwrap_err();
        assert!(e.contains(why), "{reserve} {schedule} {h}: {e}");
    }
}

/// An IR may carry vLLM's schedule as the body that writes it out, which the
/// frontend lowers to no body: the target takes that body as vLLM's too.
#[test]
fn vllms_schedule_written_out_as_a_body_is_vllm() {
    use serq::ir::{CExpr, CIter, CStageKind, CtxVar, UnOp};
    let mut p = serq::load(
        &root().join("examples/multi-turn/vllm.sq"),
        &common::horizon(10.0),
    )
    .unwrap();
    let want = serq::target::vllm(&p).unwrap();
    let st = p
        .stages
        .iter_mut()
        .find_map(|s| match &mut s.kind {
            CStageKind::Step(st) => Some(st),
            _ => None,
        })
        .unwrap();
    assert_eq!(st.iteration, None);
    st.iteration = Some(vec![
        CIter::Serve {
            only: None,
            by: None,
        },
        CIter::Admit {
            only: None,
            gate: Some(CExpr::Unary(
                UnOp::Not,
                Box::new(CExpr::Ctx(CtxVar::Preempted)),
            )),
        },
    ]);
    assert_eq!(serq::target::vllm(&p).unwrap(), want);
}
