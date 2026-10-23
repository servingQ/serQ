//! A token count is a whole number (#661, docs/design/whole-tokens.md): a
//! program whose token count may be fractional does not link, and the
//! program writes the rounding it means.
mod common;

use serq::Overrides;

fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// Chapter 4's workload as #661 found it: real-valued token counts.
const CHAPTER_4_AS_FOUND: &str = "let Lambda = 0.5;
let p = 0.8;
let Z = 3.0;

workload {
  arrive poisson(Lambda);
  init { set K = 0; }
  turn {
    set n = K == 0 ? ~uniform(1000, 3000) : ~exp(500);
    set o = ~exp(200);
    set more = ~bernoulli(p);
  }
  send (K, n, o, final: more == 0);

  session {
    loop {
      turn;
      set K = K + n + o;
      if (more) { tool (~exp(Z)); } else { end; }
    }
  }
}";

/// #661: chapter 4 hit 0.9999 with nothing evicted, because the cached
/// prefix, the sum of two releases' real lengths, read an ulp short of the
/// `K` the workload summed. As found, the pair does not link, and the
/// message says which count and where its rounding goes; with the
/// workload's rounding every hit is one.
#[test]
fn chapter_4_as_found_is_refused_and_its_hits_are_whole() {
    let deployment =
        std::fs::read_to_string(root().join("docs/tutorial/programs/04-cache.sq")).unwrap();
    let ov = Overrides {
        warmup: Some(2000.0),
        seed: Some(1),
        ..common::horizon(20000.0)
    };
    let e = common::compile(&deployment, CHAPTER_4_AS_FOUND, &ov).unwrap_err();
    for said in [
        "workload.sq: 9:13:",
        "`n` is a number of tokens (what `hold kv` takes: `cost(kv, K + n + o)`; \
         `kv` holds tokens: the `cache` of the hold at deployment.sq:22:10)",
        "`K == 0 ? ~uniform(1000, 3000) : ~exp(500)` may not be a whole number",
        "`~uniform` draws a real number",
        "write the rounding it means: `floor(…)` or `ceil(…)` (#661)",
    ] {
        assert!(e.contains(said), "want {said:?} in: {e}");
    }
    let r = common::run_example("docs/tutorial/programs/04-cache.sq", &ov).unwrap();
    let hit = r.observe("hit").unwrap();
    assert_eq!(hit.count, 35986, "{}", r.text());
    assert_eq!(hit.mean, 1.0, "{}", r.text());
    assert_eq!(r.pool("kv").unwrap().evicted_entries, 0, "{}", r.text());
}

/// The trace's `new` and `out` are numbers of tokens: a fraction there is
/// refused when the trace is read; `think` is seconds.
#[test]
fn a_fractional_trace_count_is_refused() {
    use serq::ir::trace::Corpus;
    let ok = Corpus::from_csv("session,turn,new,out,think\n0,1,2004,4,17.86\n");
    assert!(ok.is_ok(), "{ok:?}");
    for (row, column) in [
        ("0,1,2004.5,4,17.86", "column 3 (new): `2004.5`"),
        ("0,1,2004,4.25,17.86", "column 4 (out): `4.25`"),
    ] {
        let e = Corpus::from_csv(&format!("session,turn,new,out,think\n{row}\n")).unwrap_err();
        assert!(
            e.contains(column) && e.contains("not a whole number of tokens"),
            "{row}: {e}"
        );
    }
}

/// An engine and a KV pool, the server's statements `server`, the turn's
/// draw `turn`.
fn engine(server: &str, turn: &str) -> Result<serq::Program, String> {
    // a stage no statement runs does not link
    let svc = if server.contains("svc") {
        "stage svc : fifo;"
    } else {
        ""
    };
    common::compile(
        &common::main_source(&format!(
            "device gpu {{ kv cap 1e5; }}
        engine llm on gpu {{
          reqs cap 8;
          tokens cap 512;
          scheduler {{
            advance running;
            admit waiting while (running.preempted == 0)
              taking reqs (1), kv (request.known) reserve (request.known + 0.5 * holders(reqs))
              cache kv (request.input + request.output);
          }}
          worker {{ forward (1e-3); }}
        }}
        pool kv on gpu {{ evict lru; }}
        pool reqs on llm {{ }}
        pool m {{ cap 10; }}
        {svc}
        server (n) {{ {server} }}"
        )),
        &format!(
            "workload {{ arrive poisson(1); turn {{ set n = {turn}; }} send (n);
          session {{ turn; end; }} }}"
        ),
        &common::horizon(10.0),
    )
}

/// What is not a token count may be fractional: a hold's `reserve` (an
/// admission test: SGLang's scales by a real `new_token_ratio`), the units
/// of a pool that holds no token sequence, a stage's work.
#[test]
fn what_is_not_a_token_count_may_be_fractional() {
    engine(
        "hold m (cost(m, n / 1000)) { run svc (cost(svc, n / 1000)); } llm (n, 1);",
        "ceil(~exp(500))",
    )
    .expect("links");
    // the same request with a fractional input does not
    let e = engine("llm (n, 1);", "~exp(500)").unwrap_err();
    assert!(
        e.contains("`n` is a number of tokens (`llm`'s input)"),
        "{e}"
    );
    // a fraction made in the server is refused where it is written
    let e = engine("llm (ceil(n / 3) + n / 3, 1);", "ceil(~exp(500))").unwrap_err();
    assert!(
        e.contains("`llm`'s input is a number of tokens") && e.contains("(`/` divides)"),
        "{e}"
    );
}

/// The programs that write fractions where the count is not of tokens
/// link: ThunderAgent's acting-token weight below 1 shrinks a router's
/// count by a fraction, SGLang reserves by a real ratio.
#[test]
fn thunderagent_and_sglang_keep_their_fractions() {
    let mut ov = common::horizon(10.0);
    ov.set("w", "0.5").unwrap();
    common::example("examples/multi-turn/thunderagent.sq", &ov).expect("links at w = 0.5");
    common::example("examples/engines/sglang.sq", &common::horizon(10.0)).expect("links");
}

/// A pool that holds tokens has a whole cap and block.
#[test]
fn a_kv_pool_has_whole_units() {
    let e = common::compile(
        &common::main_source(
            "pool kv { cap 100.5; evict lru; }
        stage svc : fifo;
        server { hold kv (cost(kv, 10)) { run svc (cost(svc, 1)); } cache kv (cost(kv, 10)); }",
        ),
        "workload { arrive batch(1); session { turn; end; } }",
        &common::horizon(10.0),
    )
    .unwrap_err();
    assert!(
        e.contains("pool `kv` holds tokens") && e.contains("#661"),
        "{e}"
    );
}

/// A deployment and a one-request workload that sends `n`, drawn by `turn`.
fn pair(deployment: &str, turn: &str) -> Result<serq::Program, String> {
    common::compile(
        &common::main_source(deployment),
        &format!(
            "workload {{ arrive batch(1); turn {{ set n = {turn}; }} send (n);
          session {{ turn; end; }} }}"
        ),
        &common::horizon(10.0),
    )
}

fn refused(deployment: &str, said: &[&str]) {
    let e = pair(deployment, "ceil(~exp(500))").unwrap_err();
    for s in said {
        assert!(e.contains(s), "want {s:?} in: {e}");
    }
    assert!(
        e.contains("write the rounding it means: `floor(…)` or `ceil(…)` (#661)"),
        "{e}"
    );
}

/// Each way a pool comes to hold tokens is named, with its place: a spill
/// from a KV pool, a `load`, a growing run.
#[test]
fn what_makes_a_pool_hold_tokens_is_named() {
    // `kv` caches, and spills to `tier`: what a hold takes of `tier` counts
    // tokens
    refused(
        "pool kv { cap 30; evict lru; spill tier via link (size / 100) when (size >= 20); }
        pool tier { cap inf; }
        stage svc : fifo;
        stage link : fifo;
        server (n) {
          hold kv (cost(kv, n)) { run svc (cost(svc, 1)); } cache kv (cost(kv, n));
          hold tier (cost(tier, n / 2)) { run svc (cost(svc, 1)); }
        }",
        &[
            "what `hold tier` takes is a number of tokens (`tier` holds tokens: pool `kv` \
             spills to it)",
            "`cost(tier, n / 2)`",
            "(`/` divides)",
        ],
    );
    // a `load` into `m` makes its units tokens
    refused(
        "pool m { cap 1e5; }
        stage svc : fifo;
        server (n) {
          hold m (cost(m, n / 2)) { load m (cost(m, 1)); run svc (cost(svc, 1)); }
        }",
        &["what `hold m` takes is a number of tokens (`m` holds tokens: the `load` at 4:42)"],
    );
    // and so does a growing run
    refused(
        "queue llm : prefill {
          pool g { cap 1e5; }
          device gpu { kv cap 1e5; }
          engine on gpu {
            tokens cap 512;
            scheduler { advance running; admit waiting while (running.preempted == 0); }
            worker { forward (1e-3); }
          }
          pool kv on gpu { }
          prefill (prompt) {
            hold g (cost(g, prompt / 2)) { run llm prefill (cost(llm, prompt)) growing g; }
          }
        }
        server (n) { llm.prefill (n); }",
        &[
            "what `hold llm.g` takes is a number of tokens (`llm.g` holds tokens: the growing run at 11:48)",
        ],
    );
}

/// An engine's register, its `budget_left` and its connector are read as
/// the rest: a register set to a fraction makes `taking` fractional, a
/// connector's `load` and `save` count tokens, and `budget_left` of a whole
/// `tokens cap` is whole (of a fractional one, the `tokens cap` is refused
/// first).
#[test]
fn an_engines_register_budget_and_connector() {
    let engine = |state: &str, connector: &str, taking: &str| {
        format!(
            "queue lmc : l2 {{ pool kv {{ cap 1000; }} serve ps(1); }}
            device gpu {{ kv cap 1e5; }}
            pool kv on gpu {{ }}
            engine e on gpu {{
              reqs cap 8;
              tokens cap 512;
              {state}
              {connector}
              scheduler {{
                {}
                advance running;
                admit waiting while (running.preempted == 0) taking reqs (1), kv ({taking});
              }}
              worker {{ forward (1e-3); }}
            }}
            pool reqs on e {{ }}
            server (n) {{ e (n, 1); }}",
            if state.is_empty() {
                ""
            } else {
                "set r = r / 2;"
            }
        )
    };
    let none = "connector lmc { per_token (0.01); load (0); save (request.input); }";
    pair(
        &engine("", none, "min(request.known, request.hit + budget_left(e))"),
        "ceil(~exp(500))",
    )
    .expect("budget_left of a whole tokens cap is whole");
    refused(
        &engine("state r = 1;", none, "request.known + r"),
        &[
            "what engine `e` takes of `kv` is a number of tokens",
            "(`r` may not be whole)",
        ],
    );
    refused(
        &engine(
            "",
            "connector lmc { per_token (0.01); load (request.hit / 2); save (request.input); }",
            "request.known",
        ),
        &[
            "engine `e`'s connector's `load` is a number of tokens",
            "(`/` divides)",
        ],
    );
    refused(
        &engine(
            "",
            "connector lmc { per_token (0.01); load (0); save (request.input / 2); }",
            "request.known",
        ),
        &[
            "engine `e`'s connector's `save` is a number of tokens",
            "(`/` divides)",
        ],
    );
}
