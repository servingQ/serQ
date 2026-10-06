//! The IR is the definition of a program: every example program compiles to
//! IR that survives a JSON round trip unchanged, runs to the same report
//! from IR as from text, and malformed IR is rejected on load.

mod common;

use std::path::Path;

use serq::{Overrides, Program, compile_source, compile_source_at, run_ir, run_source};

fn programs() -> Vec<std::path::PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut v: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flat_map(|g| std::fs::read_dir(g.unwrap().path()).unwrap())
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "sq"))
        .collect();
    v.sort();
    assert!(v.len() >= 10, "example programs");
    v
}

/// A short run of every program, so the comparison is cheap.
fn short() -> Overrides {
    Overrides {
        horizon: Some(60.0),
        warmup: Some(0.0),
        seed: Some(3),
        ..Default::default()
    }
}

#[test]
fn json_round_trip_is_exact() {
    for path in programs() {
        let src = std::fs::read_to_string(&path).unwrap();
        let p = compile_source_at(
            &common::main_source(&src),
            path.parent(),
            &Overrides::default(),
        )
        .unwrap();
        let j = p.to_json();
        let q = Program::from_json(&j).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(j, q.to_json(), "{}", path.display());
    }
}

/// A folded constant is any double, not one a person typed:
/// `0.1 * (1 + 0.3 * 3 * 0.9 / (1 - 0.9))` is 0.9100000000000001, which serde_json's default float parser read back
/// as 0.91. The round trip needs `float_roundtrip`.
#[test]
fn a_folded_constant_survives_the_round_trip() {
    let src = "let rate = 0.1 * (1 + 0.3 * 3 * 0.9 / (1 - 0.9));
        stage svc : fifo;
        workload { arrive poisson(rate);
          session { request; end;
          }
        }
        server { run svc (~exp(1));
        }
        run { horizon 10; }";
    let j = compile_source(&common::main_source(src), &Overrides::default())
        .unwrap()
        .to_json();
    assert!(
        j.contains("0.9100000000000001"),
        "the constant is folded: {j}"
    );
    assert_eq!(j, Program::from_json(&j).unwrap().to_json());
}

#[test]
fn ir_runs_like_text() {
    for path in programs() {
        let src = std::fs::read_to_string(&path).unwrap();
        let base = path.parent();
        let from_text = run_source(&common::main_source(&src), &short(), base)
            .unwrap()
            .text();
        let p = compile_source_at(&common::main_source(&src), base, &short()).unwrap();
        let q = Program::from_json(&p.to_json()).unwrap();
        let from_ir = run_ir(&q, base).unwrap().text();
        assert_eq!(from_text, from_ir, "{}", path.display());
    }
}

/// IR read from JSON or built by a tool meets the checks a text program
/// meets in the linker: each of these used to panic the interpreter or stall
/// it, since the linker was the only one to check them (#272).
#[test]
fn ir_that_skips_the_linker_meets_its_checks() {
    use serq::ir::{CArg, CExpr, CRef, CStmt, DistKind, Fun};
    let src = "pool kv { cap 64; }
        stage engine : step { budget 8; cost 1; memory kv; }
        stage d : delay;
        workload { arrive batch(1); init { set x = 1; }
          session { request; end;
          }
        }
        server { hold kv (8) { run engine prefill (8) growing kv; }
        }
        run { horizon 10; }";
    let p = compile_source(&common::main_source(src), &Overrides::default()).unwrap();
    let d = p.stages.iter().position(|s| s.name == "d").unwrap();
    let kv = CRef {
        base: 0,
        count: 1,
        index: None,
    };
    let refused = |edit: &dyn Fn(&mut Program), said: &str| {
        let mut bad = p.clone();
        edit(&mut bad);
        let e = bad.validate().unwrap_err();
        assert!(e.contains(said), "{said}: {e}");
    };
    let run_d = CStmt::Run {
        stage: CRef {
            base: d,
            count: 1,
            index: None,
        },
        mode: serq::ir::RunMode::Plain,
        work: CExpr::Num(1.0),
        growing: None,
        also: vec![],
    };
    refused(
        &|q| q.blocks[q.init].push(run_d.clone()),
        "init: workload blocks may only `set` and `observe`",
    );
    let set =
        |e: CExpr| move |q: &mut Program| q.blocks[q.session].insert(0, CStmt::Set(0, e.clone()));
    refused(
        &set(CExpr::Call(Fun::Queue, vec![])),
        "`queue` takes 1 argument(s), got 0",
    );
    refused(
        &set(CExpr::Call(Fun::Queue, vec![CArg::Pool(kv.clone())])),
        "`queue` expects a stage here",
    );
    refused(
        &set(CExpr::Sample(DistKind::Uniform, vec![CExpr::Num(1.0)])),
        "`~uniform` takes 2 argument(s), got 1",
    );
    refused(
        &|q| q.blocks[q.session].insert(0, CStmt::Grow(kv.clone(), CExpr::Num(8.0))),
        "`grow kv` outside a hold of `kv`",
    );
    refused(
        &|q| q.blocks[q.session].insert(0, CStmt::Load(kv.clone(), CExpr::Num(1.0))),
        "`load kv` outside a hold of `kv`",
    );
    refused(
        &|q| q.blocks[q.session].insert(0, CStmt::Release(kv.clone())),
        "`release kv` outside a hold of `kv`",
    );
    // a body pointing at its own block: the walks over blocks would not end
    refused(
        &|q| {
            let s = q.session;
            q.blocks[s].insert(0, CStmt::Loop(s));
        },
        "is reached twice: a program's blocks form a tree",
    );
    // a hold over a pool that does not exist, twice: out of range first,
    // not a panic naming it (#309)
    refused(
        &|q| {
            let CStmt::Hold { pools, .. } = &mut q.blocks[q.session][0] else {
                panic!("the session holds first")
            };
            let far = CRef {
                base: 99,
                count: 1,
                index: None,
            };
            pools[0].0 = far.clone();
            let again = pools[0].clone();
            pools.push(again);
        },
        "pool reference 99..100 out of range",
    );
    let run_d_as = |mode, growing: Option<CRef>| {
        let CStmt::Run {
            stage, work, also, ..
        } = run_d.clone()
        else {
            unreachable!()
        };
        CStmt::Run {
            stage,
            mode,
            work,
            growing,
            also,
        }
    };
    let prefill_d = run_d_as(serq::ir::RunMode::Prefill, None);
    refused(
        &|q| q.blocks[q.session].insert(0, prefill_d.clone()),
        "`prefill`/`decode` are required on a step stage",
    );
    // the hold's body: `run d (1) growing kv` inside `hold kv`
    let growing_d = run_d_as(serq::ir::RunMode::Plain, Some(kv.clone()));
    refused(
        &|q| {
            let body = q.blocks[q.session]
                .iter()
                .find_map(|s| match s {
                    CStmt::Hold { body, .. } => Some(*body),
                    _ => None,
                })
                .unwrap();
            q.blocks[body].push(growing_d.clone());
        },
        "`growing` needs a step stage",
    );
}

#[test]
fn malformed_ir_is_rejected() {
    let src = std::fs::read_to_string(serq::program_path("mg1")).unwrap();
    let p = compile_source(&common::main_source(&src), &Overrides::default()).unwrap();
    let mut bad = p.clone();
    bad.version = 0;
    assert!(bad.validate().unwrap_err().contains("version"));
    let mut bad = p.clone();
    bad.session = bad.blocks.len();
    assert!(bad.validate().unwrap_err().contains("out of range"));
    let mut bad = p.clone();
    bad.blocks[bad.session].push(serq::ir::CStmt::Set(99, serq::ir::CExpr::Num(1.0)));
    assert!(bad.validate().unwrap_err().contains("attribute slot 99"));
    // a context variable outside the moment that supplies it: `tokens` exists
    // in a step stage's budget and cost, not in a session statement
    let mut bad = p.clone();
    bad.blocks[bad.session].push(serq::ir::CStmt::Set(
        0,
        serq::ir::CExpr::Ctx(serq::ir::CtxVar::Ntok),
    ));
    let e = bad.validate().unwrap_err();
    assert!(e.starts_with("session: "), "{e}");
    assert!(e.contains("`tokens` is read in a session statement"), "{e}");
    assert!(e.contains("exists only in a step stage's cost"), "{e}");
    // a hidden slot that does not exist
    let mut bad = p.clone();
    bad.hidden.push(99);
    assert!(
        bad.validate()
            .unwrap_err()
            .contains("hidden: attribute slot 99")
    );
    // what the scheduler sets cannot be hidden from it, and a slot is hidden once
    let mut bad = p.clone();
    bad.hidden.push(bad.slot_cached);
    let e = bad.validate().unwrap_err();
    assert!(e.contains("hidden `cached`: the scheduler sets it"), "{e}");
    let mut bad = p.clone();
    let slot = (0..p.attrs.len())
        .find(|&s| s != p.slot_cached && s != p.slot_computed)
        .unwrap();
    bad.hidden.push(slot);
    bad.hidden.push(slot);
    let e = bad.validate().unwrap_err();
    assert!(e.contains("twice"), "{e}");
    let j = p.to_json().replace("\"Fifo\": 1", "\"Fifo\": \"x\"");
    assert!(Program::from_json(&j).is_err());
}

/// `budget_left` reads a step engine's token budget; a stage that has none
/// used to link and fail in the run (#268). The check is the IR's, so IR
/// that bypasses the text is refused too.
#[test]
fn budget_left_needs_a_step_stage() {
    let src = "pool kv { cap 64; }
        stage engine : step { budget 8; cost 1; memory kv; }
        stage d : delay;
        workload { arrive batch(1);
          session { request; end;
          }
        }
        server { set b = budget_left(d); run d (1);
        }
        run { horizon 10; }";
    let e = compile_source(&common::main_source(src), &Overrides::default()).unwrap_err();
    assert!(e.contains("`budget_left(d)`: `d` is a delay stage"), "{e}");
    // an array is named as one
    let fam = src
        .replace("stage d : delay;", "stage d : delay; stage F[2] : fifo;")
        .replace("budget_left(d)", "budget_left(F[serial])");
    let e = compile_source(&common::main_source(&fam), &Overrides::default()).unwrap_err();
    assert!(e.contains("a member of `F` is a fifo stage"), "{e}");
    let ok = src.replace("budget_left(d)", "budget_left(engine)");
    let mut p = compile_source(&common::main_source(&ok), &Overrides::default()).unwrap();
    // the same refusal from IR: point the call at the delay stage
    use serq::ir::{CArg, CExpr, CStmt, Fun};
    let d = p.stages.iter().position(|s| s.name == "d").unwrap();
    let call = p
        .blocks
        .iter_mut()
        .flatten()
        .find_map(|s| match s {
            CStmt::Set(_, CExpr::Call(Fun::BudgetLeft, args)) => Some(args),
            _ => None,
        })
        .unwrap();
    let CArg::Stage(r) = &mut call[0] else {
        panic!("budget_left takes a stage")
    };
    r.base = d;
    let e = p.validate().unwrap_err();
    assert!(e.contains("`budget_left(d)`: `d` is a delay stage"), "{e}");
}

#[test]
fn explicit_sessions_preset_attributes() {
    // two sessions with different service times through a delay stage
    let src = r#"
        stage d : delay;
        workload { arrive batch(1); init { set w = 1; }
          session { request; end;
          }
        }
        server { run d (w); observe done = now;
        }
        run { horizon 100; }
"#;
    let p = compile_source(&common::main_source(src), &Overrides::default())
        .unwrap()
        .with_sessions(&[vec![("w", 5.0)], vec![("w", 2.0)]])
        .unwrap();
    let r = run_ir(&p, None).unwrap();
    let done = &r.observe("done").unwrap().records;
    let by_serial: Vec<(u64, f64)> = {
        let mut v: Vec<_> = done.iter().map(|&(t, s, _)| (s, t)).collect();
        v.sort_by_key(|a| a.0);
        v
    };
    assert_eq!(by_serial, vec![(0, 5.0), (1, 2.0)]);
    assert!(
        compile_source(&common::main_source(src), &Overrides::default())
            .unwrap()
            .with_sessions(&[vec![("nope", 1.0)]])
            .is_err()
    );
}

#[test]
fn inlined_trace_runs_like_the_corpus() {
    // the calibrated A100 replay over the full short-context trace (333
    // sessions): the trace file and its sessions inlined into the IR give
    // the same run
    let path = serq::program_path("vllm_replay");
    let src = std::fs::read_to_string(&path).unwrap();
    let ov = Overrides {
        horizon: Some(1500.0),
        ..Default::default()
    };
    let p = compile_source(&common::main_source(&src), &ov).unwrap();
    let base = path.parent();
    let from_trace = run_ir(&p, base).unwrap().text();
    let inlined = serq::inline_trace(p, base).unwrap();
    assert!(inlined.trace.is_none());
    let q = Program::from_json(&inlined.to_json()).unwrap();
    let from_ir = run_ir(&q, None).unwrap().text();
    assert_eq!(from_trace, from_ir);
}

/// The serving vocabulary is sugar: a program written with `admit`,
/// `prefill`, `transfer`, `decode` and `tool` compiles to the IR of the
/// same program written with `hold`, `run`, `load` and `release`.
#[test]
fn serving_forms_compile_to_the_kernel_ir() {
    let deployment = r#"
        pool memP { cap 1000; evict lru; }
        pool memD { cap 1000; }
        stage prefill : fifo;
        stage link : ps(1);
        stage decode : ps(min(present, 4));
        stage tool : delay;

        run { horizon 100; seed 1; }
    "#;
    let deployment_workload = r#"arrive poisson(0.5);
          init { set K = 0; }
          turn { set n = ~exp(100); set o = ~exp(20); set Z = ~exp(3); set T = K + n; }"#;
    let serving = format!(
        "{deployment} workload {{ {deployment_workload} session {{
            turn;
            loop {{ request;
              branch with (0.8) {{ tool Z; turn; }} else {{ end; }}
            }}

        }} }}
        server {{
          hold memP (T) {{ prefill (n + K); }} cache (T) lease memP (inf);
          hold memD (T) {{ transfer (T / 100) from memP to memD (T); decode (o); }}
          set K = T;
        }}"
    );
    let kernel = format!(
        "{deployment} workload {{ {deployment_workload} session {{
            turn;
            loop {{ request;
              branch with (0.8) {{ run tool (Z); turn; }} else {{ end; }}
            }}

        }} }}
        server {{
          hold memP (T) {{ run prefill (n + K); }} cache (T) lease memP (inf);
          hold memD (T) {{ run link (T / 100); load memD (T); release memP; run decode (o); }}
          set K = T;
        }}"
    );
    let a = compile_source(&common::main_source(&serving), &Overrides::default()).unwrap();
    let b = compile_source(&common::main_source(&kernel), &Overrides::default()).unwrap();
    assert_eq!(a.to_json(), b.to_json());
}

/// `branch with (p)` is a draw and says so. It rewrites at parse time to
/// `branch (~bernoulli(p))`: the sample is a 0 or a 1 by the time the guard
/// sees it, and a bare `branch (p)` with a fractional `p` is an error, not a
/// draw. So the sugar must be free: same IR as the explicit form.
#[test]
fn branch_with_is_sugar_for_bernoulli() {
    let head = "stage tool : delay;

        run { horizon 2000; warmup 200; seed 1; }";
    let head_workload = "arrive poisson(0.5); turn { set Z = ~exp(3); }";
    let sugar = format!(
        "{head} workload {{ {head_workload} session {{ turn; loop {{ branch with (0.8) {{ request; turn; }} else {{ end; }} }} \n}} }}\nserver {{ run tool (Z);\n}}"
    );
    let explicit = format!(
        "{head} workload {{ {head_workload} session {{ turn; loop {{ branch (~bernoulli(0.8)) {{ request; turn; }} else {{ end; }} }} \n}} }}\nserver {{ run tool (Z);\n}}"
    );
    let ov = serq::Overrides::default();
    let a = serq::compile_source(&common::main_source(&sugar), &ov).expect("the sugar compiles");
    let b = serq::compile_source(&common::main_source(&explicit), &ov)
        .expect("the explicit form compiles");
    assert_eq!(
        a.to_json(),
        b.to_json(),
        "`branch with` must reach the kernel as a bernoulli sample"
    );

    // and the same run as the explicit draw. The bare `branch (0.8)` this
    // replaced is a link error now (`tests/lints.rs`), so the corpus-wide
    // "no reported number moved" check that justified the rewrite cannot be
    // written any more - it was run once, over all sixteen programs, before
    // the lint existed.
    let one = serq::run_source(&common::main_source(&sugar), &ov, None).expect("runs");
    let two = serq::run_source(&common::main_source(&explicit), &ov, None).expect("runs");
    assert_eq!(one.text(), two.text(), "the sugar must not move a run");
}

/// A figure has to say which edges are draws. The lecture's own
/// `fig:deployment` writes "resume w.p. p" by hand.
#[test]
fn a_draw_is_labelled_w_p() {
    // a station before the branch, as every program in the corpus has: the
    // guard labels the edge that leaves it
    let src = "stage svc : fifo; stage tool : delay;
        workload { arrive poisson(0.5); turn { set Z = ~exp(3); }
          session { turn; loop { request; branch with (0.8) { run tool (Z); turn; } else { end; } }
          }
        }
        server { run svc (1);
        }
        run { horizon 100; }";
    let p = serq::compile_source(&common::main_source(src), &serq::Overrides::default()).unwrap();
    let svg = serq::view::svg::render(&serq::view::deployment::figure(&p));
    assert!(
        svg.contains("w.p. 0.8"),
        "the deployment view labels the draw"
    );
    assert!(
        !svg.contains("~bernoulli"),
        "and does not leak the desugaring"
    );
}

/// `at admission (hit = e)` names a value the hold's header is written in
/// terms of. Everything in a header is evaluated when the session is
/// admitted; a `set` above the hold is not, and looks the same - which is the
/// bug the vLLM program carried. The clause is substituted at parse time, so
/// it must reach the kernel as the inlined expression and nothing else.
#[test]
fn at_admission_is_substituted_into_the_header() {
    let head = "pool kv { cap 1e5; block 16; evict lru; }
        pool reqs { cap 8; }
        stage engine : step { budget 512; cost 1e-3; memory kv; }

        run { horizon 500; }";
    let head_workload = "arrive poisson(0.3); init { set K = 0; }
                   turn { set n = ~exp(500); set o = ~exp(200) + 1; }";
    let bound = format!(
        "{head} workload {{ {head_workload} session {{ turn; loop {{ request; end; }}
        }} }}
        server {{ set prompt = K + n;
          hold reqs (1), kv (min(prompt, hit + budget_left(engine)))
          at admission (hit = min(cachedin(kv), prompt - 1)) {{
            prefill (prompt - cached) growing kv;
          }} cache (prompt + o);
          set K = prompt + o;
        }}"
    );
    let inlined = format!(
        "{head} workload {{ {head_workload} session {{ turn; loop {{ request; end; }}
        }} }}
        server {{ set prompt = K + n;
          hold reqs (1), kv (min(prompt, min(cachedin(kv), prompt - 1) + budget_left(engine))) {{
            prefill (prompt - cached) growing kv;
          }} cache (prompt + o);
          set K = prompt + o;
        }}"
    );
    let ov = serq::Overrides::default();
    let a = serq::compile_source(&common::main_source(&bound), &ov).expect("the clause compiles");
    let b = serq::compile_source(&common::main_source(&inlined), &ov)
        .expect("the inlined form compiles");
    assert_eq!(
        a.to_json(),
        b.to_json(),
        "the clause must vanish into the header"
    );
}

/// A bare identifier argument (`min(hit, 10)`) is parsed as a reference,
/// since it may name a pool or a stage; when it names a binding it is the
/// binding. Before this the substitution skipped it and `hit` read the
/// session attribute of that name, 0 on a first admission, silently.
#[test]
fn a_bound_name_is_substituted_when_it_stands_alone_as_an_argument() {
    let head = "pool kv { cap 1e5; block 16; evict lru; }
        pool reqs { cap 8; }
        stage engine : step { budget 512; cost 1e-3; memory kv; }

        run { horizon 500; }";
    let head_workload = "arrive poisson(0.3); init { set K = 0; }
                   turn { set n = ~exp(500); set o = ~exp(200) + 1; }";
    let bound = format!(
        "{head} workload {{ {head_workload} session {{ turn; request; end;
        }} }}
        server {{ set prompt = K + n;
          hold reqs (1), kv (min(hit, 10)) at admission (hit = min(cachedin(kv), prompt - 1)) {{
            prefill (prompt - cached) growing kv;
          }} cache (prompt + o);
        }}"
    );
    let inlined = format!(
        "{head} workload {{ {head_workload} session {{ turn; request; end;
        }} }}
        server {{ set prompt = K + n;
          hold reqs (1), kv (min(min(cachedin(kv), prompt - 1), 10)) {{
            prefill (prompt - cached) growing kv;
          }} cache (prompt + o);
        }}"
    );
    let ov = serq::Overrides::default();
    let a = serq::compile_source(&common::main_source(&bound), &ov).expect("the clause compiles");
    let b = serq::compile_source(&common::main_source(&inlined), &ov)
        .expect("the inlined form compiles");
    assert_eq!(
        a.to_json(),
        b.to_json(),
        "the bare `hit` must be the binding"
    );
}

/// The bindings are substituted, so a name used twice would draw twice. That
/// is not a binding anyone means to write, and the parser says so.
#[test]
fn at_admission_rejects_a_draw() {
    let src = "pool kv { cap 100; } stage s : fifo;
        workload { arrive poisson(1);
          session { request; end;
          }
        }
        server { hold kv (x) at admission (x = ~exp(3)) { run s (1); }
        }
        run { horizon 10; }";
    let e = serq::compile_source(&common::main_source(src), &serq::Overrides::default())
        .expect_err("rejected");
    assert!(e.contains("draws a sample"), "{e}");
}

/// A later binding sees the earlier ones, so a header can be written in steps.
#[test]
fn at_admission_bindings_are_sequential() {
    let head = "pool kv { cap 1000; } stage s : fifo;

        run { horizon 10; }";
    let head_workload = "arrive poisson(1); init { set n = 10; }";
    let steps = format!(
        "{head} workload {{ {head_workload} session {{ request; end;
        }} }}
        server {{ hold kv (need) at admission (half = n / 2, need = half + 1)
          {{ run s (1); }}
        }}"
    );
    let flat = format!(
        "{head} workload {{ {head_workload} session {{ request; end; \n}} }}\nserver {{ hold kv (n / 2 + 1) {{ run s (1); }}\n}}"
    );
    let ov = serq::Overrides::default();
    assert_eq!(
        serq::compile_source(&common::main_source(&steps), &ov)
            .unwrap()
            .to_json(),
        serq::compile_source(&common::main_source(&flat), &ov)
            .unwrap()
            .to_json()
    );
}

/// `fits` was this clause's name. A program that still says it gets told what
/// happened rather than a parse error about a brace.
#[test]
fn fits_says_it_is_now_reserve() {
    let src = "pool kv { cap 100; } stage s : fifo;
        workload { arrive poisson(1);
          session { request; end;
          }
        }
        server { hold kv (1) fits (2) { run s (1); }
        }
        run { horizon 10; }";
    let e = serq::compile_source(&common::main_source(src), &serq::Overrides::default())
        .expect_err("rejected");
    assert!(e.contains("`fits` is now `reserve`"), "{e}");
}

/// `admit` was once this statement's name, then the server's `admit if`,
/// and is now only the pool option. A program that still says it is told
/// what to write, and that the pool option keeps the word.
#[test]
fn admit_as_a_statement_says_what_to_write() {
    let src = "pool kv { cap 100; } stage s : fifo;
        workload { arrive poisson(1);
          session { request; end;
          }
        }
        server { admit kv (1) { run s (1); }
        }
        run { horizon 10; }";
    let e = serq::compile_source(&common::main_source(src), &serq::Overrides::default())
        .expect_err("rejected");
    assert!(e.contains("is now `hold … at admission"), "{e}");
    assert!(
        e.contains("admit via"),
        "and says the pool option is unchanged: {e}"
    );
}

/// `workload { session { … request; … } }` with `server { … }` is the
/// expanded session. Moving the context update across the request boundary
/// without changing statement order must preserve the IR and the run.
#[test]
fn the_request_boundary_does_not_change_the_session_ir() {
    let head = "pool kv { cap 1e5; block 16; evict lru; }
        pool reqs { cap 8; }
        stage engine : step { budget 512; cost 1e-3; memory kv; }
        stage tool : delay;
        run { horizon 500; seed 1; }";
    let client = "arrive poisson(0.3); init { set K = 0; }
        turn { set n = ~exp(500); set o = ~exp(200) + 1; set more = ~bernoulli(0.9); }";
    let split = format!(
        "{head}
        workload {{ {client}
          session {{
            turn;
            loop {{
              request;
              set K = prompt + o;
              branch (more) {{ tool (~exp(3)); turn; }} else {{ end; }}
            }}
          }}
        }}
        server {{
          set t0 = now;
          set prompt = K + n;
          hold reqs (1), kv (min(prompt, hit + budget_left(engine)))
          at admission (hit = min(cachedin(kv), prompt - 1)) {{
            prefill (prompt - cached) growing kv;
            observe ttft = now - t0;
            decode (o - 1) growing kv;
          }} cache (prompt + o);
          observe response = now - t0;
        }}"
    );
    let flat = format!(
        "{head}
        workload {{ {client}
          session {{
            turn;
            loop {{ request;
              branch (more) {{ tool (~exp(3)); turn; }} else {{ end; }}
            }}

          }}
        }}
        server {{
          set t0 = now;
          set prompt = K + n;
          hold reqs (1), kv (min(prompt, hit + budget_left(engine)))
          at admission (hit = min(cachedin(kv), prompt - 1)) {{
            prefill (prompt - cached) growing kv;
            observe ttft = now - t0;
            decode (o - 1) growing kv;
          }} cache (prompt + o);
          observe response = now - t0;
          set K = prompt + o;
        }}"
    );
    let ov = serq::Overrides::default();
    let a = serq::compile_source(&common::main_source(&split), &ov).expect("the two sides compile");
    let b = serq::compile_source(&common::main_source(&flat), &ov)
        .expect("the moved context update compiles");
    assert_eq!(a.to_json(), b.to_json(), "one session, one IR");
    // and the same run: nothing downstream of the parser can tell
    let ra = serq::run_source(&common::main_source(&split), &ov, None).unwrap();
    let rb = serq::run_source(&common::main_source(&flat), &ov, None).unwrap();
    assert_eq!(ra.json(), rb.json());
}

/// `enter … keep` is `hold … cache`, and the pool option `admit via` is a
/// different keyword that the rename does not touch.
#[test]
fn enter_is_hold_and_admit_via_survives() {
    let head = "pool kv { cap 1000; } pool reqs { cap 4; admit via engine; }
        stage engine : step { budget 64; cost 1; memory kv; }

        run { horizon 20; }";
    let head_workload = "arrive poisson(1); init { set n = 10; }";
    let sugar = format!(
        "{head} workload {{ {head_workload} session {{ request; end; \n}} }}\nserver {{ hold reqs (1), kv (n) {{ prefill (n) growing kv; }} cache (n);\n}}"
    );
    let kernel = format!(
        "{head} workload {{ {head_workload} session {{ request; end; \n}} }}\nserver {{ hold reqs (1), kv (n) {{ run engine prefill (n) growing kv; }} cache (n);\n}}"
    );
    let ov = serq::Overrides::default();
    assert_eq!(
        serq::compile_source(&common::main_source(&sugar), &ov)
            .unwrap()
            .to_json(),
        serq::compile_source(&common::main_source(&kernel), &ov)
            .unwrap()
            .to_json()
    );
}

/// A label shows a number as it would have been written: folding noise goes,
/// whole numbers keep every digit, and code to paste back is exact.
#[test]
fn numbers_read_as_written() {
    use serq::ir::{show_num, show_num_exact};
    assert_eq!(show_num(0.9100000000000001), "0.91"); // vllm_subagents.sq's folded arrival rate
    assert_eq!(show_num(1e-5 * 3.0), "3e-5");
    assert_eq!(show_num(-0.91), "-0.91");
    assert_eq!(show_num(2e-9), "2e-9");
    assert_eq!(show_num(160000.0), "160000");
    assert_eq!(show_num(1234567890123.0), "1234567890123");
    assert_eq!(show_num(f64::INFINITY), "inf");
    assert_eq!(show_num(0.99999999999999), "0.99999999999999");
    assert_eq!(show_num(999999999999.5), "999999999999.5");
    assert_eq!(show_num_exact(0.9100000000000001), "0.9100000000000001");
}

/// `choose j in n by (k1, k2, …)` takes the smallest key tuple, compared
/// in order (#140): the second key decides only among the first key's ties,
/// and a tie on every key goes to the smallest index.
#[test]
fn choose_compares_its_keys_in_order() {
    let pick = |by: &str| {
        let src = format!(
            "stage s : delay;
        workload {{ arrive batch(1);
          session {{ request; end;
          }}
        }}
        server {{ choose j in 4 by ({by}); observe j = j;
        }}
        run {{ horizon 1; }}"
        );
        let r = run_source(&common::main_source(&src), &Overrides::default(), None).unwrap();
        r.observe("j").unwrap().samples[0]
    };
    // the first key ties 1 and 3; the second picks 3
    assert_eq!(pick("j == 1 || j == 3 ? 0 : 1, j == 3 ? 0 : 1"), 3.0);
    // what `* 1e9` encoded, without the bound on the second key
    assert_eq!(pick("j == 2 ? 0 : 1, -1e12 * j"), 2.0);
    // a tie everywhere goes to the smallest index
    assert_eq!(pick("0, 0"), 0.0);
    // one key is the form it was
    assert_eq!(pick("-j"), 3.0);
}

/// A v6 IR with a `choose` has a scalar `key`: it is refused by its version,
/// not by the shape of the field that changed, and an empty key list, which
/// no text can write, is refused too.
#[test]
fn an_old_or_keyless_choose_is_refused_plainly() {
    let src = "stage s : delay; workload { arrive batch(1);
          session { request; end;
          }
        }
        server { choose j in 2 by (-j);
        } run { horizon 1; }";
    let p = compile_source(&common::main_source(src), &Overrides::default()).unwrap();
    let json = p.to_json();
    let old = json
        .replacen(
            &format!("\"version\": {}", serq::ir::IR_VERSION),
            "\"version\": 6",
            1,
        )
        .replace("\"key\": [", "\"key\": ")
        .replace("]\n        }\n      }", "\n        }\n      }");
    let e = Program::from_json(&old).unwrap_err();
    assert!(e.contains("IR version 6"), "{e}");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let mut v = v;
    fn clear(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                if let Some(k) = m.get_mut("key") {
                    *k = serde_json::Value::Array(vec![]);
                }
                m.values_mut().for_each(clear);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(clear),
            _ => {}
        }
    }
    clear(&mut v);
    let e = Program::from_json(&v.to_string()).unwrap_err();
    assert!(e.contains("no key"), "{e}");
}

/// An error `Program::validate` finds in a statement of a text program
/// points at the statement, as a linker error does (#279).
#[test]
fn a_validate_error_points_at_the_statement() {
    let src = "pool kv { cap 64; }
        stage d : delay;
        workload { arrive batch(1);
          session { request;
            end;

          }
        }
        server {
          run d (1);
          load kv (1);
        }
        run { horizon 10; }";
    let e = compile_source(&common::main_source(src), &Overrides::default()).unwrap_err();
    assert!(
        e.starts_with("11:16: session: `load kv` outside a hold of `kv`"),
        "{e}"
    );
    assert!(e.contains("11 |           load kv (1);"), "{e}");
}
