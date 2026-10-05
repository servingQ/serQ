//! The shape of `Report::json`, which consumers read by field name: a
//! renamed, removed or retyped field is a change of `REPORT_VERSION`, an
//! added one is recorded here (`docs/ir.md`, Stability).

use serq::engine::report::REPORT_VERSION;
use serq::{Overrides, compile_source, run_ir};

fn keys(v: &serde_json::Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

#[test]
fn the_report_has_the_shape_its_version_names() {
    let src = "pool kv { cap 10; } stage svc : fifo;
        workload { arrive poisson(1); }
        session { hold kv (1) { run svc (~exp(0.5)); } observe x = now; end; }
        gauge g = used(kv);
        run { horizon 100; }";
    let p = compile_source(src, &Overrides::default()).unwrap();
    let j: serde_json::Value = serde_json::from_str(&run_ir(&p, None).unwrap().json()).unwrap();
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
            "end",
            "ended",
            "events",
            "gauges",
            "horizon",
            "mean_live",
            "observes",
            "pools",
            "seed",
            "serq_version",
            "stages",
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
            "index",
            "mean_cached",
            "mean_holders",
            "mean_queue",
            "mean_used",
            "mean_wait",
            "name",
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
        workload { arrive poisson(1); }
        session { set j = ~bernoulli(0.5); hold reqs (1) { hold kv[j] (1) { run svc[j] (~exp(0.5)); } } end; }
        run { horizon 100; }";
    let p = compile_source(src, &Overrides::default()).unwrap();
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
    let itl = |serve: &str| {
        let src = format!(
            "pool reqs {{ cap 2; admit via engine; }} pool kv {{ cap 20; }} stage gate : delay;
             stage engine : step {{ budget 4; chunk 4; cost 1; memory kv; {serve} }}
             workload {{ arrive batch(2); init {{ set prompt = serial == 0 ? 2 : 4; }} }}
             session {{
               run gate (2 * serial);
               hold reqs (1), kv (min(prompt, left)) reserve (prompt)
                    at admission (left = budget_left(engine)) {{
                 prefill prompt growing kv;
                 branch (serial == 0) {{ decode (3) growing kv; }}
               }}
               end;
             }}
             run {{ horizon 20; warmup 0; seed 1; }}"
        );
        let p = compile_source(&src, &Overrides::default()).unwrap();
        let r = run_ir(&p, None).unwrap();
        let s = r.stage("engine").unwrap().clone();
        (s.mean_itl, s.itl_p99)
    };
    let (mean, p99) = itl("serve exclusive prefill;");
    assert_eq!(mean, 4.0 / 3.0);
    assert!(near(p99, 2.0), "{p99}");
    let (mean, p99) = itl("");
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
            "stage p : step {{ cost 1; }} stage d : step {{ cost 1; }} stage link : delay;
             workload {{ arrive batch(1); }}
             session {{
               prefill on p (2);
               run link (5);
               {recompute}
               decode on d (2);
               end;
             }}
             run {{ horizon 20; warmup 0; seed 1; }}"
        );
        let p = compile_source(&src, &Overrides::default()).unwrap();
        let r = run_ir(&p, None).unwrap();
        let (p, d) = (r.stage("p").unwrap(), r.stage("d").unwrap());
        assert!(p.mean_itl.is_nan(), "a first token has no gap");
        d.mean_itl
    };
    assert_eq!(itl(""), 3.5);
    assert_eq!(itl("prefill on d (1);"), 1.0);
}

#[test]
fn the_gaps_add_up_to_the_decode_time_through_preemptions() {
    // One engine short of KV, so that requests are preempted and resume by
    // recomputing (vLLM: the resumed prefill samples the next token). A
    // request's gaps then add up to its last token less its first, and the
    // engine's mean gap is the token-weighted TPOT, preemptions and all.
    let src = "pool reqs { cap 64; admit via engine; }
        pool kv { cap 6000; block 16; preempt lifo; admit via engine; }
        stage engine : step { budget 2048; cost 0.001 + 1e-6 * tokens; memory kv; }
        workload {
          arrive poisson(40);
          hidden o;
          init { set prompt = floor(~uniform(500, 1500)); set o = floor(~exp(100)) + 2; }
        }
        session {
          hold kv (min(known, budget_left(engine))) reserve (known), reqs (1)
               at admission (known = computed < prompt ? prompt : computed + 1) {
            prefill (known) growing kv;
            branch (known == prompt) { set first = now; }
            decode (o - 1 - (known - prompt)) growing kv;
          }
          observe span = now - first;
          observe gaps = o - 1;
          end;
        }
        run { horizon 1e6; warmup 0; seed 1; arrivals 8000; }";
    let p = compile_source(src, &Overrides::default()).unwrap();
    let r = run_ir(&p, None).unwrap();
    assert!(
        r.pool("kv").unwrap().preemptions > 0,
        "no preemption to test"
    );
    let sum = |name: &str| r.observe(name).unwrap().samples.iter().sum::<f64>();
    let tpot = sum("span") / sum("gaps");
    let itl = r.stage("engine").unwrap().mean_itl;
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
        workload { arrive poisson(1); }
        session { hold reqs (1) { hold kv[0] (1) { run svc[0] (~exp(2)); } } end; }
        run { horizon 100; }";
    let p = compile_source(src, &Overrides::default()).unwrap();
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
        queue D[ND] : decode {
          pool kv { cap 100; }
          serve step { cost 1; memory kv; }
          decode (p) { hold kv (p) { prefill (p) growing kv; } }
        }
        workload { arrive batch(1); init { set prompt = 4; } session { request gw; end; } }
        run { horizon 10; }";
    let p = compile_source(src, &Overrides::default()).unwrap();
    let r = run_ir(&p, None).unwrap();
    assert_eq!(r.pools_named("D.kv")[0].index, Some(0));
    assert_eq!(r.stages_named("D")[0].index, Some(0));
}

#[test]
fn a_step_stage_reports_what_its_iterations_carried() {
    // examples/single-turn/separate_phases.sq: with the serve clause the
    // iterations are A:p2, B:p4, A:d1, A:d1 of a unit each; without it,
    // A:p2, then A:d1+B:p3 and A:d1+B:p1 mixed.
    let src = |serve: &str| {
        format!(
            "pool reqs {{ cap 2; admit via engine; }} pool kv {{ cap 20; }} stage gate : delay;
             stage engine : step {{ budget 4; chunk 4; cost 1; memory kv; {serve} }}
             workload {{ arrive batch(2); init {{ set prompt = serial == 0 ? 2 : 4; }} }}
             session {{
               run gate (serial);
               hold reqs (1), kv (min(prompt, left)) reserve (prompt)
                    at admission (left = budget_left(engine)) {{
                 prefill prompt growing kv;
                 branch (serial == 0) {{ decode (2) growing kv; }}
               }}
               end;
             }}
             run {{ horizon 20; warmup 0; seed 1; }}"
        )
    };
    let stage = |serve: &str| {
        let p = compile_source(&src(serve), &Overrides::default()).unwrap();
        let r = run_ir(&p, None).unwrap();
        let s = r.stage("engine").unwrap().clone();
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
        stage("serve exclusive prefill;"),
        (0.1, 0.1, 0.0, 0.1, 1.0, 1.0)
    );
    assert_eq!(stage(""), (0.05, 0.0, 0.1, 0.1, 1.0, 1.0));
}

/// #232: the report says which serq produced it.
#[test]
fn the_report_records_the_serq_version() {
    let p = compile_source(
        "stage svc : delay; workload { arrive batch(1); } session { run svc (1); end; } run { horizon 2; }",
        &Overrides::default(),
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
        ..Default::default()
    };
    let src = include_str!("../examples/pd-disaggregation/pd_batching.sq");
    let p = compile_source(src, &ov).unwrap();
    let r = run_ir(&p, None).unwrap();
    assert!(
        r.pool("D.kv").unwrap().preemptions > 0,
        "no preemption to test"
    );
    let sum = |name: &str| r.observe(name).unwrap().samples.iter().sum::<f64>();
    let gaps = r
        .observe("output_tokens")
        .unwrap()
        .samples
        .iter()
        .map(|o| o - 1.0)
        .sum::<f64>();
    let tpot = sum("decode_time") / gaps;
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
    let src = "def below(x) = x < 0;
        stage svc : delay; workload { arrive batch(40); }
        session {
          run svc (serial);
          observe never = serial < 0;
          observe neither = !(serial >= 0);
          observe both = serial < 0 && serial > 100;
          observe defined = below(serial);
          observe odd = serial - 2 * floor(serial / 2) == 1;
          observe always = serial >= 0;
          observe zero = 0;
          observe cond = serial < 0 ? 1 : 0;
          branch (serial < 20) { observe mixed = serial < 0; } else { observe mixed = 0; }
          branch (serial < 10) { observe few = serial > 100; }
          end;
        }
        run { horizon 100; }";
    let r = run_ir(&compile_source(src, &Overrides::default()).unwrap(), None).unwrap();
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

/// The note is a line of text, not a lint error, because one shipped program
/// earns it by design: `vllm_single_turn.sq` never reads its cache back, so
/// its `hit` (from `lib/vllm.sq`) is 0 all run long. Every other example and
/// tutorial program runs without a note; a new one that earns it shows up
/// here, to be judged.
#[test]
fn the_corpus_earns_one_note() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut programs = vec![];
    for dir in std::fs::read_dir(root.join("examples")).unwrap().flatten() {
        for f in std::fs::read_dir(dir.path()).unwrap().flatten() {
            programs.push(f.path());
        }
    }
    for f in std::fs::read_dir(root.join("docs/tutorial/programs"))
        .unwrap()
        .flatten()
    {
        programs.push(f.path());
    }
    programs.retain(|p| p.extension().is_some_and(|e| e == "sq"));
    programs.sort();
    assert!(programs.len() >= 25, "{programs:?}");
    let mut noted = vec![];
    // nor does any reject a session (the `rej:` note, #271), nor end with an
    // engine idle with work (the `idle:` note, #355)
    let mut rejected = vec![];
    for p in &programs {
        let r = serq::run_file(p, &Overrides::default())
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let name = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
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
    assert_eq!(
        noted,
        [(
            "examples/single-turn/vllm_single_turn.sq".to_string(),
            "hit".to_string()
        )]
    );
}
