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
        ("examples/vendors/rbln.sq", "serves exclusive prefill"),
        (
            "examples/single-turn/fastertransformer.sq",
            "has its own iteration",
        ),
        ("examples/single-turn/mg1.sq", "vLLM is one step engine"),
        (
            "examples/engines/tensorrt_llm.sq",
            "does not preempt as vLLM does",
        ),
        (
            "examples/pd-disaggregation/vllm_nixl_push.sq",
            "vLLM is one step engine",
        ),
    ] {
        let e = target(path).unwrap_err();
        assert!(
            e.contains("not on vLLM's architecture") && e.contains(why),
            "{path}: {e}"
        );
    }
    let src = "pool kv { cap 160; block 16; evict lru; preempt lifo; } pool reqs { cap 4; }
        stage engine : step { budget 64; cost 1; serve by (remaining); memory kv; }
        workload { arrive batch(1); init { set n = 8; }
          session { turn; end;
          }
        }
        server { hold reqs (1), kv (n) { prefill (n) growing kv; }
        } ";
    let p = serq::compile_source(&common::main_source(src), &common::horizon(10.0)).unwrap();
    let e = serq::target::vllm(&p).unwrap_err();
    assert!(
        e.contains("serves by `remaining`") && e.contains("observes `decoding` and `admission`"),
        "{e}"
    );
}

/// Keys vLLM's scheduler can observe go to the programmable scheduler,
/// `tools/serq_vllm.py`, as the IR writes them.
#[test]
fn observable_serve_keys_name_the_programmable_scheduler() {
    let src = "pool kv { cap 160; block 16; evict lru; preempt lifo; } pool reqs { cap 4; }
        stage engine : step { budget 64; cost 1; serve by (decoding ? 0 : 1, -admission); memory kv; }
        workload { arrive batch(1); init { set n = 8; }
          session { turn; end;
          }
        }
        server { hold reqs (1), kv (n) { prefill (n) growing kv; }
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
/// constant cap is not vLLM's: the target takes `long_prefill(reqs, c)` or 0.
#[test]
fn a_constant_chunk_cap_is_refused_and_the_rule_taken() {
    let src = std::fs::read_to_string(root().join("examples/multi-turn/vllm.sq")).unwrap();
    let dir = root().join("examples/multi-turn");
    let rule = "chunk long_prefill(reqs, chunk_cap);";
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
        &common::main_source(&src.replace(rule, "chunk 512;")),
        Some(&dir),
        &common::horizon(10.0),
    )
    .unwrap();
    let e = serq::target::vllm(&p).unwrap_err();
    assert!(
        e.contains("the chunk cap is `512`") && e.contains("long_prefill(reqs, c)"),
        "{e}"
    );
}

/// What vLLM's scheduler does not have since the iteration became a body:
/// a body of its own (and with it a register), a held reservation, a
/// request's legs.
/// vLLM's body written out is vLLM.
#[test]
fn the_newer_constructs_are_refused_and_vllms_body_taken() {
    let base = "pool kv { cap 160; block 16; evict lru; preempt lifo; }
        pool reqs { cap 4; admit via engine; RESERVE }
        stage engine : step { budget 64; cost 1; memory kv; ITER }
        workload { arrive batch(1); init { set n = 8; }
          session { turn; HOLD end;
          }
        }
        server {
        } ";
    let hold = "hold reqs (1), kv (n) { prefill (n) growing kv; }";
    let compile = |reserve: &str, iter: &str, h: &str| {
        let src = base
            .replace("RESERVE", reserve)
            .replace("ITER", iter)
            .replace("HOLD", h);
        serq::compile_source(&common::main_source(&src), &common::horizon(10.0)).unwrap()
    };
    let body = "iteration { serve; admit while (!preempted); }";
    serq::target::vllm(&compile("", body, hold)).unwrap();
    let legs = "fork { hold reqs (1), kv (n) { prefill (n) growing kv; } } join;";
    for (reserve, iter, h, why) in [
        (
            "",
            "iteration { serve; admit; }",
            hold,
            "has its own iteration",
        ),
        // a register is set by a body, so a program with one has its own
        (
            "",
            "state k = 0; iteration { serve; admit while (!preempted); set k = k + 1; }",
            hold,
            "has its own iteration",
        ),
        ("reserve held;", "", hold, "holds its reservations"),
        ("", "", legs, "fork"),
    ] {
        let e = serq::target::vllm(&compile(reserve, iter, h)).unwrap_err();
        assert!(e.contains(why), "{reserve} {iter} {h}: {e}");
    }
}
