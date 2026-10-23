//! The shape of `Report::json`, which consumers read by field name: a
//! renamed, removed or retyped field is a change of `REPORT_VERSION`, an
//! added one is recorded here (`docs/ir.md`, Stability).

mod common;

use serq::engine::report::REPORT_VERSION;
use serq::{Overrides, run_ir};

/// One single-turn session at a time, arriving at rate 1.
const POISSON: &str = "workload { arrive poisson(1);
          session { turn; end;
          }
        }
";

/// Two sessions at t = 0: serial 0 (A) with a prompt of 2, serial 1 (B) of 4.
const TWO: &str =
    "workload { arrive batch(2); init { set prompt = serial == 0 ? 2 : 4; } send (prompt);
          session { turn;
            end;

          }
        }
";

/// One single-turn session.
const ONE: &str = "workload { arrive batch(1);
          session { turn;
            end;

          }
        }
";

fn keys(v: &serde_json::Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

/// The sum of an observation's samples.
fn total(r: &serq::Report, name: &str) -> f64 {
    r.observe(name).unwrap().samples.iter().sum()
}

#[test]
fn the_report_has_the_shape_its_version_names() {
    let src = "pool kv { cap 10; } stage svc : fifo;
        server { hold kv (cost(kv, 1)) { run svc (cost(svc, ~exp(0.5))); } observe x = now;
        }
        gauge g = used(kv);
        ";
    let p = common::compile(&common::main_source(src), POISSON, &common::horizon(100.0)).unwrap();
    let j: serde_json::Value = serde_json::from_str(&run_ir(&p, None).unwrap().json()).unwrap();
    // the client's metrics, each shaped as an observation
    assert_eq!(keys(&j["client"]), vec!["response", "tpot", "ttft"]);
    assert_eq!(keys(&j["client"]["response"]), keys(&j["observes"]["x"]));
    // every engine request's status (`docs/design/request-state.md`),
    // visited or not, with its stays and the requests in it
    assert_eq!(
        keys(&j["status"]),
        vec![
            "finished",
            "preempted",
            "running",
            "waiting",
            "waiting_for_kv"
        ]
    );
    assert_eq!(
        keys(&j["status"]["waiting"]),
        vec!["mean", "number", "p50", "p99", "stays"]
    );
    // one object per engine that takes requests: none here
    assert_eq!(j["engines"], serde_json::json!({}));
    let shape = (
        keys(&j),
        keys(&j["observes"]["x"]),
        keys(&j["gauges"]["g"]),
        keys(&j["stages"][0]),
        keys(&j["pools"][0]),
    );
    let want = (
        vec![
            "arrivals",
            "client",
            "end",
            "ended",
            "engines",
            "events",
            "gauges",
            "horizon",
            "mean_live",
            "observes",
            "pools",
            "seed",
            "serq_version",
            "stages",
            "status",
            "turns",
            "warmup",
        ],
        vec!["ci", "count", "cv2", "mean", "p99"],
        vec!["ci", "max", "mean", "min"],
        vec![
            "completed",
            "decode_only",
            "idle_with_work",
            "index",
            "iterations",
            "itl_p50",
            "itl_p99",
            "mean_decode_batch",
            "mean_decode_step",
            "mean_decodes",
            "mean_itl",
            "mean_number",
            "mean_service",
            "mean_wait",
            "mixed",
            "name",
            "prefill_only",
            "throughput",
            "utilization",
        ],
        vec![
            "admissions",
            "evicted_entries",
            "evicted_units",
            "growing_at_end",
            "growing_stalled",
            "index",
            "mean_cached",
            "mean_holders",
            "mean_queue",
            "mean_used",
            "mean_wait",
            "name",
            "over_cap",
            "preemptions",
            "rejected",
            "spills",
            "stuck",
        ],
    );
    let want = (
        want.0.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        want.1.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        want.2.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        want.3.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        want.4.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    );
    assert_eq!(
        shape, want,
        "the report's shape moved: a renamed, removed or retyped field bumps REPORT_VERSION ({REPORT_VERSION}); an added one is recorded here; say so in the release"
    );
}

/// #201: an array's members are told apart by their index, in the JSON
/// (`index`), the text (`kv[1]`) and `pools_named`; a single pool or stage
/// has none.
#[test]
fn an_array_member_is_reported_with_its_index() {
    let src = "pool kv[2] { cap 10; } pool reqs { cap 4; } stage svc[2] : fifo;
        server { set j = ~bernoulli(0.5); hold reqs (cost(reqs, 1)) { hold kv[j] (cost(kv, 1)) { run svc[j] (cost(svc, ~exp(0.5))); } }
        }
        ";
    let p = common::compile(&common::main_source(src), POISSON, &common::horizon(100.0)).unwrap();
    let r = run_ir(&p, None).unwrap();
    let j: serde_json::Value = serde_json::from_str(&r.json()).unwrap();
    let rows = |k: &str| -> Vec<(String, serde_json::Value)> {
        j[k].as_array()
            .unwrap()
            .iter()
            .map(|x| (x["name"].as_str().unwrap().to_string(), x["index"].clone()))
            .collect()
    };
    assert_eq!(
        rows("pools"),
        vec![
            ("kv".into(), 0.into()),
            ("kv".into(), 1.into()),
            ("reqs".into(), serde_json::Value::Null),
        ]
    );
    assert_eq!(
        rows("stages"),
        vec![("svc".into(), 0.into()), ("svc".into(), 1.into())]
    );
    let text = r.text();
    for row in ["kv[0] ", "kv[1] ", "reqs ", "svc[0] ", "svc[1] "] {
        assert!(
            text.lines().any(|l| l.starts_with(row)),
            "no row `{row}`:\n{text}"
        );
    }
    let named: Vec<Option<u32>> = r.pools_named("kv").iter().map(|p| p.index).collect();
    assert_eq!(named, vec![Some(0), Some(1)]);
    assert_eq!(r.pools_named("reqs")[0].index, None);
}

/// A quantile is its bucket's middle, within 0.5 % of the value.
fn near(x: f64, want: f64) -> bool {
    (x - want).abs() <= 0.005 * want
}

#[test]
fn the_gaps_between_tokens_count_a_prefill_that_cuts_in() {
    // A (prompt 2, three decodes) from t = 0, B (prompt 4, none) from t = 2,
    // unit steps. Exclusive: A:p2, A:d1, B:p4, A:d1, A:d1, so A's tokens
    // come at 1 (the prefill's end, its first), 2, 4, 5: gaps 1, 2, 1.
    // Mixed: A:p2, A:d1, A:d1+B:p3, A:d1+B:p1: tokens at 1, 2, 3, 4, gaps
    // 1, 1, 1. The gaps add up to the last token less the first.
    let itl = |schedule: &str| {
        let src = format!(
            "stage gate : delay;
        device gpu {{ kv cap 20; }}
        engine llm on gpu {{
          reqs cap 2;
          tokens cap 4;
          scheduler {{ {schedule} }}
          worker {{ forward (1); }}
        }}
        pool reqs on llm {{ }}
        pool kv on gpu {{ }}
        server (prompt) {{
          run gate (cost(gate, 2 * serial));
          llm (prompt, serial == 0 ? 4 : 1);   // A decodes three after its prefill's token
        }}
        "
        );
        let p = common::compile(
            &common::main_source(&src),
            TWO,
            &Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(20.0)
            },
        )
        .unwrap();
        let r = run_ir(&p, None).unwrap();
        let s = r.stage("llm").unwrap().clone();
        (s.mean_itl, s.itl_p99)
    };
    let (mean, p99) = itl(
        "exclusive prefill each at most (4); admit waiting while (running.preempted == 0) each at most (4) taking reqs (1), kv (min(request.input, budget_left(llm))) reserve (request.input);",
    );
    assert_eq!(mean, 4.0 / 3.0);
    assert!(near(p99, 2.0), "{p99}");
    let (mean, p99) = itl(
        "advance running each at most (4); admit waiting while (running.preempted == 0) each at most (4) taking reqs (1), kv (min(request.input, budget_left(llm))) reserve (request.input);",
    );
    assert_eq!(mean, 1.0);
    assert!(near(p99, 1.0), "{p99}");
}

#[test]
fn a_gap_holds_the_transfer_between_two_engines() {
    // A prefill engine's step ends at 1 (the first token), a read of 5,
    // then the decode engine: its tokens at 7 and 8, gaps 6 and 1, both
    // the decode engine's. If the decode engine first recomputes the last
    // prompt token (llmd_nixl_pull.sq), that token, at 7, is the first, and
    // the decodes at 8 and 9 have gaps 1 and 1.
    let itl = |recompute: &str| {
        let src = format!(
            "device gpu_p {{ }}
        device gpu_d {{ }}
        engine p on gpu_p {{
          tokens cap inf;
          scheduler {{ advance running; admit waiting while (running.preempted == 0); }}
          worker {{ forward (1); }}
        }}
        engine d on gpu_d {{
          tokens cap inf;
          scheduler {{ advance running; admit waiting while (running.preempted == 0); }}
          worker {{ forward (1); }}
        }}
        stage link : delay;
        server {{
          p (2, 1);
          run link (cost(link, 5));
          d ({recompute});
        }}
        "
        );
        let p = common::compile(
            &common::main_source(&src),
            ONE,
            &Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(20.0)
            },
        )
        .unwrap();
        let r = run_ir(&p, None).unwrap();
        let (p, d) = (r.stage("p").unwrap(), r.stage("d").unwrap());
        assert!(p.mean_itl.is_nan(), "a first token has no gap");
        d.mean_itl
    };
    // no prefill: two decodes; or one token recomputed, then two decodes
    assert_eq!(itl("0, 2"), 3.5);
    assert_eq!(itl("1, 3"), 1.0);
}

#[test]
fn the_gaps_add_up_to_the_decode_time_through_preemptions() {
    // One engine short of KV, so that requests are preempted and resume by
    // recomputing (vLLM: the resumed prefill samples the next token). A
    // request's gaps then add up to its last token less its first, and the
    // engine's mean gap is the token-weighted TPOT, preemptions and all.
    let src = "device gpu { kv cap 6000; }
        engine llm on gpu {
          reqs cap 64;
          tokens cap 2048;
          scheduler {
            advance running;
            admit waiting while (running.preempted == 0)
              taking kv (min(request.known, budget_left(llm))) reserve (request.known), reqs (1);
          }
          worker { forward (0.001 + 1e-6 * batch.tokens); }
        }
        pool reqs on llm { }
        pool kv on gpu { block 16; preempt lifo; }
        server (prompt, hidden o) {
          llm (prompt, o);
          observe span = now - llm.first;
          observe gaps = o - 1;
        }
        ";
    let p = common::compile(
        &common::main_source(src),
        "workload {
          arrive poisson(40);
          init { set prompt = floor(~uniform(500, 1500)); set o = floor(~exp(100)) + 2; }
          send (prompt, o);

          session { turn;
            end;

          }
        }
",
        &Overrides {
            warmup: Some(0.0),
            seed: Some(1),
            arrivals: Some(8000),
            ..common::horizon(1e6)
        },
    )
    .unwrap();
    let r = run_ir(&p, None).unwrap();
    assert!(
        r.pool("kv").unwrap().preemptions > 0,
        "no preemption to test"
    );
    let tpot = total(&r, "span") / total(&r, "gaps");
    let itl = r.stage("llm").unwrap().mean_itl;
    // every session drains (`arrivals`), so every gap is observed: exact
    assert!(
        (itl - tpot).abs() <= 1e-9 * tpot,
        "ITL {itl} against TPOT {tpot} preempt {}",
        r.pool("kv").unwrap().preemptions
    );
}

/// #214: a one-member array is an array, as the program writes it
/// (`kv[0]`), so a sweep over its size keeps the label at N = 1.
#[test]
fn a_one_member_array_keeps_its_index() {
    let src = "pool kv[1] { cap 10; } pool reqs { cap 4; } stage svc[1] : fifo;
        server { hold reqs (cost(reqs, 1)) { hold kv[0] (cost(kv, 1)) { run svc[0] (cost(svc, ~exp(2))); } }
        }
        ";
    let p = common::compile(&common::main_source(src), POISSON, &common::horizon(100.0)).unwrap();
    let r = run_ir(&p, None).unwrap();
    assert_eq!(r.pools_named("kv")[0].index, Some(0));
    assert_eq!(r.pools_named("reqs")[0].index, None);
    assert_eq!(r.stages[0].index, Some(0));
    let text = r.text();
    assert!(text.lines().any(|l| l.starts_with("kv[0] ")), "{text}");
    assert!(text.lines().any(|l| l.starts_with("svc[0] ")), "{text}");
}

/// A queue family of one is a family (`D[j].decode`), so its pools and
/// stages are reported as `D[0]`, as the program calls them.
#[test]
fn a_queue_family_of_one_is_reported_by_index() {
    let src = "let ND = 1;
        queue gw : gateway { route { choose j in ND by (0); D[j].decode (prompt); } }
        queue D[ND] : agg {
          device gpu { kv cap 100; }
          engine on gpu { tokens cap inf; scheduler { advance running; admit waiting while (running.preempted == 0); } worker { forward (1); } }
          pool kv on gpu { }
          decode (p) { hold kv (cost(kv, p)) { run D prefill (cost(D, p)) growing kv; } }
        }
        server (prompt) { gw.route(); }
        ";
    let p = common::compile(&common::main_source(src), "workload { arrive batch(1); init { set prompt = 4; } send (prompt); session { turn; end; } }", &common::horizon(10.0))
    .unwrap();
    let r = run_ir(&p, None).unwrap();
    assert_eq!(r.pools_named("D.kv")[0].index, Some(0));
    assert_eq!(r.stages_named("D")[0].index, Some(0));
}

#[test]
fn a_step_stage_reports_what_its_iterations_carried() {
    // examples/single-turn/separate_phases.sq: with the serve clause the
    // iterations are A:p2, B:p4, A:d1, A:d1 of a unit each; without it,
    // A:p2, then A:d1+B:p3 and A:d1+B:p1 mixed.
    let src = |schedule: &str| {
        format!(
            "stage gate : delay;
        device gpu {{ kv cap 20; }}
        engine llm on gpu {{
          reqs cap 2;
          tokens cap 4;
          scheduler {{ {schedule} }}
          worker {{ forward (1); }}
        }}
        pool reqs on llm {{ }}
        pool kv on gpu {{ }}
        server (prompt) {{
          run gate (cost(gate, serial));
          llm (prompt, serial == 0 ? 3 : 1);   // A decodes two after its prefill's token
        }}
        "
        )
    };
    let stage = |schedule: &str| {
        let p = common::compile(
            &common::main_source(&src(schedule)),
            TWO,
            &Overrides {
                warmup: Some(0.0),
                seed: Some(1),
                ..common::horizon(20.0)
            },
        )
        .unwrap();
        let r = run_ir(&p, None).unwrap();
        let s = r.stage("llm").unwrap().clone();
        (
            s.prefill_only,
            s.decode_only,
            s.mixed,
            s.mean_decodes,
            s.mean_decode_batch,
            s.mean_decode_step,
        )
    };
    assert_eq!(
        stage(
            "exclusive prefill each at most (4); admit waiting while (running.preempted == 0) each at most (4) taking reqs (1), kv (min(request.input, budget_left(llm))) reserve (request.input);"
        ),
        (0.1, 0.1, 0.0, 0.1, 1.0, 1.0)
    );
    assert_eq!(
        stage(
            "advance running each at most (4); admit waiting while (running.preempted == 0) each at most (4) taking reqs (1), kv (min(request.input, budget_left(llm))) reserve (request.input);"
        ),
        (0.05, 0.0, 0.1, 0.1, 1.0, 1.0)
    );
}

/// #232: the report says which serq produced it.
#[test]
fn the_report_records_the_serq_version() {
    let p = common::compile(
        &common::main_source("stage svc : delay; server { run svc (cost(svc, 1));\n} "),
        "workload { arrive batch(1); \n  session { turn; end; \n  }\n}",
        &common::horizon(2.0),
    )
    .unwrap();
    let j: serde_json::Value = serde_json::from_str(&run_ir(&p, None).unwrap().json()).unwrap();
    assert_eq!(j["serq_version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn a_decoders_gaps_add_up_through_its_preemptions() {
    // examples/pd-disaggregation/pd_batching.sq split, its decode engine
    // short of KV: requests are preempted there, before and after their
    // recompute of the last prompt token (the client's first). Every
    // session drains, so the decode engine's mean gap is the token-weighted
    // TPOT exactly.
    let ov = Overrides {
        lets: [("mode", "1"), ("blocksD", "512"), ("Lambda", "40")]
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    serq::frontend::parser::parse_expr(v).unwrap(),
                )
            })
            .collect(),
        defs: vec![("prompt_len".into(), "2000".into())],
        warmup: Some(0.0),
        arrivals: Some(4000),
        ..common::horizon(300.0)
    };
    let r = common::run_example("examples/pd-disaggregation/pd_batching.sq", &ov).unwrap();
    assert!(
        r.pool("D.kv").unwrap().preemptions > 0,
        "no preemption to test"
    );
    let gaps = r
        .observe("output_tokens")
        .unwrap()
        .samples
        .iter()
        .map(|o| o - 1.0)
        .sum::<f64>();
    let tpot = total(&r, "decode_time") / gaps;
    let itl = r.stage("D").unwrap().mean_itl;
    assert!(
        (itl - tpot).abs() <= 1e-9 * tpot,
        "ITL {itl} against TPOT {tpot}"
    );
}

/// #232: a test (`c > 0`, `S == hit`, `!x`) that was 0 over 40 or more
/// samples gets a note under the table. The table alone does not show it
/// (cv2 is NaN for a constant 0), and an always-0 `hit` is how #230 was
/// found, late. `batch(40)` arrives 40 sessions at once, one sample each,
/// `serial` 0..39: `never` is 0 for every serial, as are `neither` (`!`),
/// `both` (`&&`) and `defined` (a test inside a `def`); `odd` is 1 for the
/// 20 odd serials, `always` is 1 for all (a test that held is not a
/// finding), `zero` is a literal, not a test (an event counted is written
/// `= 1`), `cond` is 0 but by `?:`, whose outermost operator is not a test,
/// `mixed` is a test at one site and a literal at the other, and `few` is 0
/// but only for the 10 serials below 10.
#[test]
fn a_test_observe_that_never_held_is_noted() {
    let src = "def below(x) { x < 0 }
        stage svc : delay;
        server {
          run svc (cost(svc, serial));
          observe never = serial < 0;
          observe neither = !(serial >= 0);
          observe both = serial < 0 && serial > 100;
          observe defined = below(serial);
          observe odd = serial - 2 * floor(serial / 2) == 1;
          observe always = serial >= 0;
          observe zero = 0;
          observe cond = serial < 0 ? 1 : 0;
          if (serial < 20) { observe mixed = serial < 0; } else { observe mixed = 0; }
          if (serial < 10) { observe few = serial > 100; }
        }
        ";
    let r = run_ir(
        &common::compile(
            &common::main_source(src),
            "workload { arrive batch(40);
          session { turn;
            end;

          }
        }
",
            &common::horizon(100.0),
        )
        .unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(r.observe("never").unwrap().count, 40);
    assert_eq!(r.observe("few").unwrap().count, 10);
    let t = r.text();
    for name in ["never", "neither", "both", "defined"] {
        assert!(
            t.contains(&format!(
                "note: observe {name} is constant 0 over 40 samples"
            )),
            "{name}: {t}"
        );
    }
    for name in ["odd", "always", "zero", "cond", "mixed", "few"] {
        assert!(
            !t.contains(&format!("observe {name} is constant")),
            "{name}: {t}"
        );
    }
}

/// A growth that waits for room a hold around it on the same pool keeps
/// (#238): the outer hold takes 1008 of 2015, the inner one grows from 16
/// to 1008, and with `preempt none` and no cache it waits for ever. The
/// report said `stuck 0` and an `idle:` note that blamed the schedule.
#[test]
fn a_growth_left_waiting_is_noted() {
    let src = "device gpu { kv cap 2015; }
        engine llm on gpu {
          tokens cap 8192;
          scheduler {
            advance running;
            admit waiting while (running.preempted == 0)
              taking kv (request.hit + 8) reuse (992) cache (1000);
          }
          worker { forward (1e-4 + 1e-5 * batch.tokens); }
        }
        pool kv on gpu { block 16; evict lru; }
        server {
          hold kv (cost(kv, 1000)) { llm (1000, 1); }
        }
        ";
    let r = run_ir(
        &common::compile(
            &common::main_source(src),
            "workload { arrive batch(1); session { loop { turn; } } }",
            &common::horizon(20.0),
        )
        .unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(r.pools[0].growing_at_end, 1);
    assert!(r.pools[0].growing_stalled);
    assert!(
        r.text().contains(
            "grow: 1 hold(s) wait to grow in pool `kv` when the run ends, and every holder of \
             `kv` is one of them"
        ),
        "{}",
        r.text()
    );
}

/// Every example and tutorial program under every workload it runs, to
/// `horizon(100)`, named `program with workload`.
fn corpus() -> Vec<(String, serq::Report)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut programs = vec![];
    for dir in std::fs::read_dir(root.join("examples")).unwrap().flatten() {
        // (`examples/pairs.json` is a file beside the groups)
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for f in files.flatten() {
            programs.push(f.path());
        }
    }
    for f in std::fs::read_dir(root.join("docs/tutorial/programs"))
        .unwrap()
        .flatten()
    {
        programs.push(f.path());
    }
    // a workload's file is not a program: it runs beside the one it is paired with
    let workloads: Vec<_> = programs
        .iter()
        .flat_map(|p| serq::paired_workloads(p))
        .collect();
    programs.retain(|p| p.extension().is_some_and(|e| e == "sq") && !workloads.contains(p));
    programs.sort();
    assert!(programs.len() >= 25, "{programs:?}");
    // each under every workload it runs
    let runs = programs.iter().flat_map(|p| {
        let ws = serq::paired_workloads(p);
        assert!(!ws.is_empty(), "{}: no workload", p.display());
        ws.into_iter().map(move |w| (p, w))
    });
    runs.map(|(p, w)| {
        let r = serq::load_paired(p, &w, &common::horizon(100.0))
            .and_then(|ir| run_ir(&ir, w.parent()))
            .unwrap_or_else(|e| panic!("{} with {}: {e}", p.display(), w.display()));
        let name = format!(
            "{} with {}",
            p.strip_prefix(root).unwrap().display(),
            w.strip_prefix(root).unwrap().display()
        );
        (name, r)
    })
    .collect()
}

/// The note is a line of text, not a lint error, because a shipped program
/// may earn it by design (vllm.sq under `single_turn.sq` earned it for its `hit`,
/// which it no longer observes: the engine records what each admission
/// reused, `vllm.reused`, a count and no test). Every example and tutorial
/// program runs without a note; a new one that earns it shows up here, to
/// be judged.
#[test]
fn the_corpus_earns_one_note() {
    let mut noted = vec![];
    // nor does any reject a session (the `rej:` note, #271), nor end with an
    // engine idle with work (the `idle:` note, #355)
    let mut rejected = vec![];
    for (name, r) in corpus() {
        for o in r.observes.iter().filter(|o| o.never_held()) {
            noted.push((name.clone(), o.name.clone()));
        }
        for s in r.stages.iter().filter(|s| s.idle_with_work) {
            rejected.push((name.clone(), format!("idle {}", s.name)));
        }
        for q in r.pools.iter().filter(|q| q.rejected > 0) {
            rejected.push((name.clone(), q.name.clone()));
        }
    }
    assert_eq!(rejected, Vec::<(String, String)>::new());
    assert_eq!(noted, Vec::<(String, String)>::new());
}

// ------------------------------------------------- the engine table ----
// (#594, `docs/design/engine-admissions.md`)

/// Engine `e` on a GPU of 1000 KV tokens that reads from its cache every
/// token but the last and caches `cache`, with `connector` as written (to
/// the L2 `lmc` of 1000, declared with it); one iteration a second.
fn one_engine(connector: &str, cache: &str) -> String {
    let l2 = if connector.is_empty() {
        ""
    } else {
        "queue lmc : l2 { pool kv { cap 1000; } serve ps(1); }"
    };
    format!(
        "{l2}
        device gpu {{ kv cap 1000; }}
        pool kv on gpu {{ evict lru; }}
        engine e on gpu {{
          tokens cap inf;
          {connector}
          scheduler {{
            advance running;
            admit waiting while (running.preempted == 0)
              taking kv (request.known) reuse (request.known - 1) cache ({cache});
          }}
          worker {{ forward (1); }}
        }}
        server (input) {{ e (input, 1); }}
        "
    )
}

/// One session of two turns, of 100 then 150 tokens.
const TWO_TURNS: &str = "workload {
          arrive batch(1);
          init { set K = 0; }
          send (input: K == 0 ? 100 : 150);
          session { turn; set K = 1; tool (1); turn; end; }
        }
";

/// The first turn reads nothing and computes its 100 tokens; the second
/// reads the 100 the first cached and computes 50. The engine's row gives
/// the share of the 250 tokens the cache gave, 100.
#[test]
fn an_engine_reports_its_admissions_and_token_hit() {
    let r = common::run(
        &common::main_source(&one_engine("", "request.input + request.output")),
        TWO_TURNS,
        &common::horizon(100.0),
    )
    .unwrap();
    assert_eq!(r.observe("e.reused").unwrap().samples, [0.0, 100.0]);
    assert_eq!(r.observe("e.prefill").unwrap().samples, [100.0, 50.0]);
    let e = r.engine("e").unwrap();
    assert_eq!((e.admissions, e.readmissions), (2, 0));
    assert_eq!(e.token_hit, 100.0 / 250.0);
    let j: serde_json::Value = serde_json::from_str(&r.json()).unwrap();
    assert_eq!(
        keys(&j["engines"]["e"]),
        ["admissions", "readmissions", "token_hit"]
    );
    assert_eq!(j["engines"]["e"]["token_hit"], 0.4);
    let text = r.text();
    assert!(
        text.contains("\nengine  admits  readmits  token hit\n")
            && text
                .lines()
                .any(|l| l == "e            2         0      0.400"),
        "{text}"
    );
}

/// The open question of #594: a token a connector loaded is among the
/// admission's tokens, not its hits. The engine caches 40 of the first
/// turn's 100 and its L2 all 100: the second turn reads 40, loads 60 and
/// computes 50. The engine's own cache gave 40 of the 250 tokens, not 40
/// of the 40 + 150 its records `e.reused` and `e.prefill` hold, which
/// leave the loaded tokens out (0.21).
#[test]
fn a_loaded_token_is_among_the_tokens_not_the_hits() {
    let r = common::run(
        &common::main_source(&one_engine(
            "connector lmc {
               per_token (0.01);
               load (max(0, min(cachedin(lmc.kv), request.known - 1) - request.hit));
               save (request.input);
             }",
            "40",
        )),
        TWO_TURNS,
        &common::horizon(100.0),
    )
    .unwrap();
    assert_eq!(r.observe("e.reused").unwrap().samples, [0.0, 40.0]);
    assert_eq!(r.observe("e.loaded").unwrap().samples, [0.0, 60.0]);
    assert_eq!(r.observe("e.prefill").unwrap().samples, [100.0, 50.0]);
    let e = r.engine("e").unwrap();
    assert_eq!(e.admissions, 2);
    assert_eq!(e.token_hit, 40.0 / 250.0);
}

/// Two requests that outgrow 48 tokens of KV (`tests/connector.rs`, without
/// the connector): one is preempted once and admitted again. The row counts
/// three admissions, one a re-admission, which ends the one stay in
/// `preempted`.
#[test]
fn a_readmission_is_counted_apart() {
    let r = common::run(
        &common::main_source(
            "device gpu { kv cap 48; }
             pool kv on gpu { block 16; evict lru; preempt lifo; }
             engine e on gpu {
               tokens cap 64;
               scheduler { advance running; admit waiting while (running.preempted == 0) taking kv (request.known); }
               worker { forward (1); }
             }
             server { e (16, 20); }",
        ),
        "workload { arrive batch(2); }",
        &common::horizon(1000.0),
    )
    .unwrap();
    assert_eq!(r.pool("kv").unwrap().preemptions, 1, "{}", r.text());
    let e = r.engine("e").unwrap();
    assert_eq!((e.admissions, e.readmissions), (3, 1));
    let preempted = r.statuses.iter().find(|x| x.name == "preempted").unwrap();
    assert_eq!(preempted.stays, 1);
}

/// The run of #594 as it was filed: the row's admissions are `E.reused`'s
/// count (1862 then) and, without a connector, its token hit what each
/// reader of the records computed, `Σ E.reused / (Σ E.reused +
/// Σ E.prefill)` (0.795). Invariants, not values: a change to the run
/// moves the numbers, not these.
///
/// Its re-admissions equal the stays in `preempted` (10 and 10) on this
/// run, not by construction: the table counts admissions, the status table
/// the stays it closes, two counts that agree here
/// (`docs/design/engine-admissions.md`, Known wrinkles). A change to the
/// run may part them and fail this assertion with no fault in either.
#[test]
fn the_issue_594_run_agrees_with_its_records() {
    let mut ov = Overrides {
        warmup: Some(300.0),
        ..common::horizon(3000.0)
    };
    for (k, v) in [("N", "96"), ("S", "2048"), ("tr", "1")] {
        ov.set(k, v).unwrap();
    }
    let r = common::run_example("examples/multi-turn/thunderagent.sq", &ov).unwrap();
    let e = r.engine("E").unwrap();
    assert_eq!(e.admissions, r.observe("E.reused").unwrap().count);
    let (reused, prefill) = (total(&r, "E.reused"), total(&r, "E.prefill"));
    let hit = reused / (reused + prefill);
    assert!((e.token_hit - hit).abs() <= 1e-12, "{} {hit}", e.token_hit);
    // this run's agreement, not an invariant (above)
    let preempted = r.statuses.iter().find(|x| x.name == "preempted").unwrap();
    assert!(e.readmissions > 0, "no re-admission to test");
    assert_eq!(e.readmissions, preempted.stays);
}

/// Every engine of the corpus: a row for each engine with records, in
/// their order; its admissions its records' count; its token hit a share,
/// NaN only when no admission had a token, without a connector what the
/// readers computed from the records, and with one between the bounds its
/// records set. An admission's tokens are at most what it read, loaded and
/// computes, and at least what it read and computes: a load that brings
/// every token has the last computed again (`load_ended`), so the three
/// may count that token twice. Hence
/// `Σ reused / (Σ reused + Σ loaded + Σ prefill) <= token_hit <=
/// Σ reused / (Σ reused + Σ prefill)`.
#[test]
fn every_engine_row_agrees_with_its_records() {
    let (mut rows, mut connected) = (0, 0);
    for (name, r) in corpus() {
        let recorded: Vec<&str> = r
            .observes
            .iter()
            .filter_map(|o| o.name.strip_suffix(".reused"))
            .collect();
        let engines: Vec<&str> = r.engines.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(engines, recorded, "{name}");
        for e in &r.engines {
            rows += 1;
            let record = |what: &str| format!("{}.{what}", e.name);
            assert_eq!(
                e.admissions,
                r.observe(&record("reused")).unwrap().count,
                "{name}: {}",
                e.name
            );
            assert!(e.readmissions <= e.admissions, "{name}: {}", e.name);
            let (u, p, h) = (
                total(&r, &record("reused")),
                total(&r, &record("prefill")),
                e.token_hit,
            );
            if u + p == 0.0 {
                assert!(h.is_nan(), "{name}: {} {h}", e.name);
                continue;
            }
            assert!((0.0..=1.0).contains(&h), "{name}: {} {h}", e.name);
            if r.observe(&record("loaded")).is_none() {
                assert!(
                    (h - u / (u + p)).abs() <= 1e-9,
                    "{name}: {} {h} against {}",
                    e.name,
                    u / (u + p)
                );
            } else {
                let l = total(&r, &record("loaded"));
                let (low, high) = (u / (u + l + p), u / (u + p));
                assert!(
                    low <= h + 1e-12 && h <= high + 1e-12,
                    "{name}: {} {h} outside [{low}, {high}]",
                    e.name
                );
                connected += 1;
            }
        }
    }
    assert!(rows >= 20, "{rows} rows");
    assert!(connected >= 2, "{connected} rows with a connector");
}
