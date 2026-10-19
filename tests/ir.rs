//! The IR is the definition of a program: every example program compiles to
//! IR that survives a JSON round trip unchanged, runs to the same report
//! from IR as from text, and malformed IR is rejected on load.

mod common;

use std::path::Path;

use serq::{Overrides, Program, run_ir};

fn programs() -> Vec<std::path::PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut v: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|g| g.unwrap().path())
        // the workloads' files are not programs: `examples/pairs.json` pairs
        // them with the programs that run them
        .filter(|g| g.is_dir() && g.file_name().is_some_and(|n| n != "workloads"))
        .flat_map(|g| std::fs::read_dir(g).unwrap())
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "sq"))
        .collect();
    v.sort();
    assert!(v.len() >= 10, "example programs");
    v
}

/// The workload file `examples/pairs.json` pairs the program at `path`
/// with; a relative trace is read beside it.
fn workload(path: &Path) -> std::path::PathBuf {
    serq::paired_workload(path).unwrap_or_else(|| panic!("{}: no pair", path.display()))
}

/// The program at `path` compiled with its workload.
fn compiled(path: &Path, ov: &Overrides) -> Program {
    serq::load_paired(path, &workload(path), ov)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A short run of every program, so the comparison is cheap.
fn short() -> Overrides {
    Overrides {
        horizon: Some(60.0),
        warmup: Some(0.0),
        seed: Some(3),
        ..common::horizon(10.0)
    }
}

#[test]
fn json_round_trip_is_exact() {
    for path in programs() {
        let p = compiled(&path, &common::horizon(10.0));
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
    let src = "stage svc : fifo;
        server { run svc (cost(svc, ~exp(1)));
        }
        ";
    let workload = "let rate = 0.1 * (1 + 0.3 * 3 * 0.9 / (1 - 0.9));
        workload { arrive poisson(rate);
          session { turn; end;
          }
        }
        ";
    let ov = common::horizon(10.0);
    let j = common::compile(&common::main_source(src), workload, &ov)
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
        let w = workload(&path);
        let (src, wsrc) = (read(&path), read(&w));
        let names = (path.to_str().unwrap(), w.to_str().unwrap());
        let ov = short();
        let base = w.parent();
        let from_text = serq::run_paired(&src, &wsrc, names, &ov, base)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
            .text();
        let p = compiled(&path, &ov);
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
    let src = "device gpu { kv cap 64; }
        engine llm on gpu {
          tokens cap 8;
          scheduler { advance running; admit waiting while (running.preempted == 0) taking kv (8); }
          worker { forward (1); }
        }
        pool kv on gpu { }
        pool m { cap 10; }
        stage d : delay;
        server { hold m (cost(m, 1)) { llm (8, 1); } run d (cost(d, 1));
        }
        ";
    let workload = "workload { arrive batch(1); init { set x = 1; }
          session { turn; end;
          }
        }
        ";
    let ov = common::horizon(10.0);
    let p = common::compile(&common::main_source(src), workload, &ov).unwrap();
    let d = p.stages.iter().position(|s| s.name == "d").unwrap();
    let m = CRef {
        base: p.pools.iter().position(|q| q.name == "m").unwrap(),
        count: 1,
        index: None,
    };
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
        share: None,
        latency: None,
        poster: None,
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
            let CStmt::Hold { pools, .. } = &mut q.blocks[q.session][1] else {
                panic!("the turn is followed by a hold")
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
    refused(
        &|q| {
            let serq::ir::CStageKind::Step(st) = &mut q.stages[0].kind else {
                panic!("the engine is stage 0")
            };
            st.request.as_mut().expect("the engine's request").pools[0].0 = CRef {
                base: 99,
                count: 1,
                index: None,
            };
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
            share: None,
            latency: None,
            poster: None,
        }
    };
    let mut posted_d = run_d.clone();
    if let CStmt::Run { poster, .. } = &mut posted_d {
        *poster = Some(0);
    }
    refused(
        &|q| q.blocks[q.session].insert(0, posted_d.clone()),
        "a poster is one of the stages of a copy over several",
    );
    let prefill_d = run_d_as(serq::ir::RunMode::Prefill, None);
    refused(
        &|q| q.blocks[q.session].insert(0, prefill_d.clone()),
        "`prefill`/`decode` are required on an engine",
    );
    // the hold's body: `run d (1) growing m` inside `hold m`
    let growing_d = run_d_as(serq::ir::RunMode::Plain, Some(m.clone()));
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
        "`growing` needs an engine",
    );
}

/// An IR reader has no `tokens cap` to look for: the moment's error says
/// when the expression is read, so it holds for IR loaded from JSON too.
#[test]
fn a_moment_error_on_loaded_ir_says_when() {
    let path = serq::program_path("vllm");
    let mut p = compiled(&path, &common::horizon(10.0));
    let st = p
        .stages
        .iter_mut()
        .find_map(|s| match &mut s.kind {
            serq::ir::CStageKind::Step(st) => Some(st),
            _ => None,
        })
        .expect("a step stage");
    st.budget = serq::ir::CExpr::Ctx(serq::ir::CtxVar::Ntok);
    let e = Program::from_json(&p.to_json()).unwrap_err();
    assert!(
        e.contains(
            "`tokens` is read in an engine's `tokens cap` or `each at most`, \
             read before the iteration is formed, but it exists only in"
        ),
        "{e}"
    );
    assert!(e.contains("read after the iteration is formed"), "{e}");
}

#[test]
fn malformed_ir_is_rejected() {
    let path = serq::program_path("mg1");
    let p = compiled(&path, &common::horizon(10.0));
    let mut bad = p.clone();
    bad.version = 0;
    assert!(bad.validate().unwrap_err().contains("version"));
    let mut bad = p.clone();
    bad.session = bad.blocks.len();
    assert!(bad.validate().unwrap_err().contains("out of range"));
    let mut bad = p.clone();
    bad.blocks[bad.session].push(serq::ir::CStmt::Set(99, serq::ir::CExpr::Num(1.0)));
    assert!(bad.validate().unwrap_err().contains("attribute slot 99"));
    // a gateway's route names statements the program has, and some (#706)
    for (block, from, to) in [
        (bad.blocks.len(), 0, 1),
        (p.session, 0, 0),
        (p.session, 0, 999),
    ] {
        let mut bad = p.clone();
        bad.routes.push(serq::ir::Route {
            gateway: "gw".into(),
            block,
            from,
            to,
        });
        let e = bad.validate().unwrap_err();
        assert!(e.contains("gateway `gw`'s route names statements"), "{e}");
    }
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
    assert!(e.contains("exists only in an engine's `worker`"), "{e}");
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
    let src = "device gpu { kv cap 64; }
        engine llm on gpu {
          tokens cap 8;
          scheduler { advance running; admit waiting while (running.preempted == 0); }
          worker { forward (1); }
        }
        pool kv on gpu { }
        stage d : delay;
        server { set b = budget_left(d); run d (cost(d, 1));
        }
        ";
    let ov = common::horizon(10.0);
    let workload = "workload { arrive batch(1);
          session { turn; end;
          }
        }
        ";
    let e = common::compile(&common::main_source(src), workload, &ov).unwrap_err();
    assert!(e.contains("`budget_left(d)`: `d` is a delay stage"), "{e}");
    // an array is named as one
    let fam = src
        .replace("stage d : delay;", "stage d : delay; stage F[2] : fifo;")
        .replace("budget_left(d)", "budget_left(F[serial])");
    let e = common::compile(&common::main_source(&fam), workload, &ov).unwrap_err();
    assert!(e.contains("a member of `F` is a fifo stage"), "{e}");
    // the engine is run too, or a stage no statement runs would not link
    let ok = src
        .replace("budget_left(d)", "budget_left(llm)")
        .replace("run d (cost(d, 1));", "run d (cost(d, 1)); llm (8, 1);");
    let mut p = common::compile(&common::main_source(&ok), workload, &ov).unwrap();
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
        server (w) { run d (cost(d, w)); observe done = now;
        }

"#;
    let workload = r#"
        workload { arrive batch(1); init { set w = 1; } send (w);
          session { turn; end;
          }
        }
"#;
    let ov = common::horizon(100.0);
    let p = common::compile(&common::main_source(src), workload, &ov)
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
        common::compile(&common::main_source(src), workload, &ov)
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
    let ov = Overrides {
        horizon: Some(1500.0),
        ..common::horizon(10.0)
    };
    let p = compiled(&path, &ov);
    let w = workload(&path);
    let base = w.parent();
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


    "#;
    // the client's own station, which its session runs
    let deployment_workload = r#"stage tool : delay; workload { arrive poisson(0.5);
          init { set K = 0; }
          turn { set n = ceil(~exp(100)); set o = ~exp(20); set Z = ~exp(3); set T = K + n; }
          send (K, n, o, T);"#;
    let serving = format!(
        "{deployment}
        server (K, n, o, T) {{
          hold memP (cost(memP, T)) {{ run prefill (cost(prefill, n + K)); }} cache (cost(memP, T)) lease memP (inf);
          hold memD (cost(memD, T)) {{ transfer (T / 100) from memP to memD (T); run decode (cost(decode, o)); }}
        }}"
    );
    let serving_workload = format!(
        "{deployment_workload} session {{
            loop {{ turn; set K = T;
              if (~bernoulli(0.8)) {{ tool Z; }} else {{ end; }}
            }}

        }} }}"
    );
    let kernel = format!(
        "{deployment}
        server (K, n, o, T) {{
          hold memP (cost(memP, T)) {{ run prefill (cost(prefill, n + K)); }} cache (cost(memP, T)) lease memP (inf);
          hold memD (cost(memD, T)) {{ run link (cost(link, T / 100)); load memD (cost(memD, T)); release memP; run decode (cost(decode, o)); }}
        }}"
    );
    let kernel_workload = format!(
        "{deployment_workload} session {{
            loop {{ turn; set K = T;
              if (~bernoulli(0.8)) {{ run tool (cost(tool, Z)); }} else {{ end; }}
            }}

        }} }}"
    );
    let a = common::compile(
        &common::main_source(&serving),
        &serving_workload,
        &Overrides {
            seed: Some(1),
            ..common::horizon(100.0)
        },
    )
    .unwrap();
    let b = common::compile(
        &common::main_source(&kernel),
        &kernel_workload,
        &Overrides {
            seed: Some(1),
            ..common::horizon(100.0)
        },
    )
    .unwrap();
    assert_eq!(a.to_json(), b.to_json());
}

/// The workload's `tool` is the client's time, not a resource of the
/// deployment (#500): with no stage `tool` declared, it runs on a delay the
/// parser supplies after the declared stages, which links to the IR and the
/// run of the program that declares `stage tool : delay;` last.
#[test]
fn a_workload_tool_runs_on_its_own_delay() {
    let src = "stage svc : fifo; server { run svc (cost(svc, 1)); }";
    let body = "workload { arrive poisson(0.5); turn { set Z = ~exp(3); }
          session { loop { turn; if (~bernoulli(0.8)) { tool (Z); } else { end; } } }
        }";
    let declared = format!("stage tool : delay; {body}");
    let ov = Overrides {
        seed: Some(1),
        ..common::horizon(200.0)
    };
    let src = common::main_source(src);
    let a = common::compile(&src, &declared, &ov).unwrap();
    let b = common::compile(&src, body, &ov).expect("nothing to declare");
    assert_eq!(a.to_json(), b.to_json());
    let one = common::run(&src, &declared, &ov).unwrap();
    let two = common::run(&src, body, &ov).unwrap();
    assert_eq!(one.text(), two.text());
}

/// A declared stage `tool` is still the one `tool Z;` uses, whatever its
/// kind: a tool server with capacity is the deployment's.
#[test]
fn a_declared_fifo_tool_is_still_used() {
    let src = "stage svc : fifo;
        server { run svc (cost(svc, 1)); }";
    let workload = "stage tool : fifo;
        workload { arrive poisson(0.5); turn { set Z = ~exp(3); }
          session { loop { turn; if (~bernoulli(0.8)) { tool (Z); } else { end; } } }
        }";
    let ov = common::horizon(10.0);
    let p = common::compile(&common::main_source(src), workload, &ov).unwrap();
    let tools: Vec<_> = p.stages.iter().filter(|s| s.name == "tool").collect();
    assert_eq!(tools.len(), 1, "no second, implicit `tool`");
    assert!(!tools[0].kind.is_delay(), "the declared fifo, not a delay");
}

/// The workload's delay stands in only where nothing is declared: a stage
/// `tool` below the call would be a second, and a server's or an indexed
/// `tool` still names a declared stage.
#[test]
fn the_workload_delay_stands_in_only_for_an_undeclared_tool() {
    let svc = "stage svc : fifo; server { run svc (cost(svc, 1)); }";
    let refused = |src: &str, workload: &str| {
        common::compile(&common::main_source(src), workload, &common::horizon(10.0))
            .expect_err(workload)
    };
    let below = "workload { arrive batch(1); session { turn; tool (1); turn; } }
        stage tool : fifo;";
    let e = refused(svc, below);
    assert!(e.contains("declared below"), "{e}");
    // the supplied stage's name is no other declaration's (#483)
    let pool = "stage svc : fifo; pool tool { cap 4; }
        server { run svc (cost(svc, 1)); }";
    let e = refused(
        pool,
        "workload { arrive batch(1); session { turn; tool (1); turn; } }",
    );
    assert!(
        e.contains("runs on a stage `tool` the parser supplies, and `tool` is a pool at"),
        "{e}"
    );
    let server = "stage svc : fifo;
        server { run svc (cost(svc, 1)); tool (1); }";
    let e = refused(server, "workload { arrive batch(1); }");
    assert!(e.contains("no stage declared above plays `tool`"), "{e}");
    let e = refused(
        svc,
        "workload { arrive batch(1); session { turn; tool[0] (1); turn; } }",
    );
    assert!(e.contains("no stage declared above plays `tool`"), "{e}");
}

/// `delay` is `ps(present)`, every job at rate 1. The two spellings link to
/// one IR, so the interpreter, the drawing and the Lean generator cannot
/// tell them apart (#396).
#[test]
fn delay_is_ps_of_present() {
    let ir = |kind: &str| {
        let src = format!(
            "stage d : {kind}; stage f : fifo(1);
            server (s) {{ run d (cost(d, s)); run f (cost(f, 0.5)); }}"
        );
        let workload =
            "workload { arrive renewal(1); init { set s = floor(~uniform(1, 4)); } send (s); }";
        let ov = common::horizon(10.0);
        common::compile(&common::main_source(&src), workload, &ov).unwrap()
    };
    let (delay, ps) = (ir("delay"), ir("ps(present)"));
    assert!(delay.stages[0].kind.is_delay());
    assert_eq!(delay.to_json(), ps.to_json());
    assert!(!ir("ps(1)").stages[0].kind.is_delay());
}

/// `else if` is the `else` block holding one `if`: the same IR as the
/// nested form, in a session (`tests/iteration_body.rs` has the schedule).
#[test]
fn else_if_is_the_nested_if() {
    let src = "stage svc : delay;
            server { run svc (cost(svc, 1)); }";
    let session = |arms: &str| {
        format!(
            "workload {{ arrive poisson(0.5); turn {{ set k = ~bernoulli(0.5) + ~bernoulli(0.5); }}
              session {{ loop {{ turn; {arms} }} }} }}"
        )
    };
    let ov = common::horizon(100.0);
    let flat = session("if (k == 0) { end; } else if (k == 1) { tool (1); } else { tool (2); }");
    let nested =
        session("if (k == 0) { end; } else { if (k == 1) { tool (1); } else { tool (2); } }");
    let a = common::compile(&common::main_source(src), &flat, &ov).expect("`else if` compiles");
    let b =
        common::compile(&common::main_source(src), &nested, &ov).expect("the nested form compiles");
    assert_eq!(
        a.to_json(),
        b.to_json(),
        "`else if` must be the nested `if`"
    );
}

/// The retired `branch` says what it is now, in a session and as a name.
#[test]
fn branch_is_retired_for_if() {
    let src = common::main_source(
        "stage svc : delay;
            server { run svc (cost(svc, 1)); }",
    );
    let workload = |body: &str| {
        format!("workload {{ arrive poisson(0.5); session {{ loop {{ turn; {body} }} }} }}")
    };
    for body in [
        "branch (1) { end; }",
        "branch with (0.5) { end; }",
        "set branch = 1; end;",
    ] {
        let e = common::compile(&src, &workload(body), &common::horizon(100.0)).expect_err(body);
        assert!(e.contains("is now `if (e)`"), "{body}: {e}");
    }
}

/// A figure has to say which edges are draws. The lecture's own
/// `fig:deployment` writes "resume w.p. p" by hand.
#[test]
fn a_draw_is_labelled_w_p() {
    // a station before the branch, as every program in the corpus has: the
    // guard labels the edge that leaves it; the runs are the session's, so
    // the view draws the session
    let src = "server {}
        ";
    let workload = "stage svc : fifo; stage tool : delay;
        workload { arrive poisson(0.5); turn { set Z = ~exp(3); }
          session {  loop { turn; run svc (cost(svc, 1));
              if (~bernoulli(0.8)) { run tool (cost(tool, Z));  } else { end; } }
          }
        }
        ";
    let ov = common::horizon(100.0);
    let p = common::compile(&common::main_source(src), workload, &ov).unwrap();
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

/// A workload that sends nothing, one turn a session.
const POISSON: &str = "workload { arrive poisson(1); session { turn; end; } }";

/// The bindings are substituted, so a name used twice would draw twice. That
/// is not a binding anyone means to write, and the parser says so.
#[test]
fn at_admission_rejects_a_draw() {
    let src = "pool kv { cap 100; } stage s : fifo;
        server { hold kv (cost(kv, x)) at admission (x = ~exp(3)) { run s (cost(s, 1)); }
        }
        ";
    let ov = common::horizon(10.0);
    let e = common::compile(&common::main_source(src), POISSON, &ov).expect_err("rejected");
    assert!(e.contains("draws a sample"), "{e}");
}

/// A later binding sees the earlier ones, so a header can be written in steps.
#[test]
fn at_admission_bindings_are_sequential() {
    let head = "pool kv { cap 1000; } stage s : fifo;

        ";
    let head_workload =
        "workload { arrive poisson(1); init { set n = 10; } send (n); session { turn; end; } }";
    let steps = format!(
        "{head}
        server (n) {{ hold kv (cost(kv, need)) at admission (half = n / 2, need = half + 1)
          {{ run s (cost(s, 1)); }}
        }}"
    );
    let flat = format!(
        "{head}\nserver (n) {{ hold kv (cost(kv, n / 2 + 1)) {{ run s (cost(s, 1)); }}\n}}"
    );
    let ov = common::horizon(10.0);
    assert_eq!(
        common::compile(&common::main_source(&steps), head_workload, &ov)
            .unwrap()
            .to_json(),
        common::compile(&common::main_source(&flat), head_workload, &ov)
            .unwrap()
            .to_json()
    );
}

/// `fits` was this clause's name. A program that still says it gets told what
/// happened rather than a parse error about a brace.
#[test]
fn fits_says_it_is_now_reserve() {
    let src = "pool kv { cap 100; } stage s : fifo;
        server { hold kv (cost(kv, 1)) fits (2) { run s (cost(s, 1)); }
        }
        ";
    let ov = common::horizon(10.0);
    let e = common::compile(&common::main_source(src), POISSON, &ov).expect_err("rejected");
    assert!(e.contains("`fits` is now `reserve`"), "{e}");
}

/// `admit` was once this statement's name, then the server's `admit if`.
/// A program that still says it is told what to write.
#[test]
fn admit_as_a_statement_says_what_to_write() {
    let src = "pool kv { cap 100; } stage s : fifo;
        server { admit kv (1) { run s (cost(s, 1)); }
        }
        ";
    let ov = common::horizon(10.0);
    let e = common::compile(&common::main_source(src), POISSON, &ov).expect_err("rejected");
    assert!(e.contains("is now `hold … at admission"), "{e}");
}

/// `workload { session { … turn; … } }` with `server { … }` is the
/// expanded session. Moving the context update across the request boundary
/// without changing statement order must preserve the IR and the run.
#[test]
fn the_request_boundary_preserves_size_ownership() {
    let head = "device gpu { kv cap 1e5; }
        engine llm on gpu {
          reqs cap 8;
          tokens cap 512;
          scheduler {
            advance running;
            admit waiting while (running.preempted == 0)
              taking reqs (1), kv (min(request.input, request.hit + budget_left(llm)))
              reuse (request.input - 1)
              cache (request.input + request.output);
          }
          worker { forward (1e-3); }
        }
        pool kv on gpu { block 16; evict lru; }
        pool reqs on llm { }
        ";
    let client = "stage tool : delay; workload { arrive poisson(0.3); init { set K = 0; }
        turn { set n = ceil(~exp(500)); set o = floor(~exp(200)) + 1; set more = ~bernoulli(0.9); }
        send (K, n, o);";
    let split = format!(
        "{head}
        server (K, n, o) reply (prompt) {{
          set t0 = now;
          set prompt = K + n;
          llm (prompt, o);
          observe ttft = llm.first - t0;
          observe response = now - t0;
        }}"
    );
    let split_client = format!(
        "{client}
          session {{

            loop {{
              turn;
              set K = reply.prompt + o;
              if (more) {{ tool (~exp(3));  }} else {{ end; }}
            }}
          }}
        }}"
    );
    let flat = format!(
        "{head}
        server (K, n, o) {{
          set t0 = now;
          set prompt = K + n;
          llm (prompt, o);
          observe ttft = llm.first - t0;
          observe response = now - t0;
          set K = prompt + o;
        }}"
    );
    let flat_client = format!(
        "{client}
          session {{

            loop {{ turn;
              if (more) {{ tool (~exp(3));  }} else {{ end; }}
            }}

          }}
        }}"
    );
    let ov = Overrides {
        seed: Some(1),
        ..common::horizon(500.0)
    };
    let a = common::compile(&common::main_source(&split), &split_client, &ov)
        .expect("the two sides compile");
    let error = common::compile(&common::main_source(&flat), &flat_client, &ov)
        .expect_err("moving a client size update into the server changes the contract");
    assert!(
        error.contains("server cannot assign request size `K`"),
        "{error}"
    );
    let restored = Program::from_json(&a.to_json()).unwrap();
    assert_eq!(
        run_ir(&a, None).unwrap().json(),
        run_ir(&restored, None).unwrap().json()
    );
}

/// A label shows a number as it would have been written: folding noise goes,
/// whole numbers keep every digit, and code to paste back is exact.
#[test]
fn numbers_read_as_written() {
    use serq::ir::{show_num, show_num_exact};
    assert_eq!(show_num(0.9100000000000001), "0.91"); // subagents.sq's folded arrival rate
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

/// One session of one turn.
const BATCH: &str = "workload { arrive batch(1);
          session { turn; end;
          }
        }
        ";

/// `choose j in n by (k1, k2, …)` takes the smallest key tuple, compared
/// in order (#140): the second key decides only among the first key's ties,
/// and a tie on every key goes to the smallest index.
#[test]
fn choose_compares_its_keys_in_order() {
    let pick = |by: &str| {
        let src = format!(
            "server {{ choose j in 4 by ({by}); observe picked = j;
        }}
        "
        );
        let ov = common::horizon(1.0);
        let r = common::run(&common::main_source(&src), BATCH, &ov).unwrap();
        r.observe("picked").unwrap().samples[0]
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
    let src = "server { choose j in 2 by (-j);
        } ";
    let ov = common::horizon(1.0);
    let p = common::compile(&common::main_source(src), BATCH, &ov).unwrap();
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
        server {
          run d (cost(d, 1));
          load kv (cost(kv, 1));
        }
        ";
    let ov = common::horizon(10.0);
    let e = common::compile(&common::main_source(src), BATCH, &ov).unwrap_err();
    assert!(
        e.starts_with("deployment.sq: 5:16: session: `load kv` outside a hold of `kv`"),
        "{e}"
    );
    assert!(e.contains("5 |           load kv (cost(kv, 1));"), "{e}");
}

/// A KV relation's copy names its poster, the side whose worker starts it
/// (`kvt.nixl_pull`: the reader's NIC, the run's `also`; `kvt.nixl_push`:
/// the source's, its `stage`); a `transfer on` written by hand names none.
#[test]
fn a_relations_copy_names_its_poster() {
    use serq::ir::CStmt;
    let posters = |name: &str| -> Vec<Option<usize>> {
        let path = serq::program_path(name);
        let p = compiled(&path, &short());
        // a connector's copy is its load's transfer, a hand-written one a run
        let copies = p
            .connectors()
            .filter_map(|(_, c)| c.load.as_ref())
            .filter(|t| !t.also.is_empty())
            .map(|t| t.poster);
        p.blocks
            .iter()
            .flatten()
            .filter_map(|s| match s {
                CStmt::Run { also, poster, .. } if !also.is_empty() => Some(*poster),
                _ => None,
            })
            .chain(copies)
            .collect()
    };
    let pull = posters("llmd_nixl_pull");
    assert!(
        !pull.is_empty() && pull.iter().all(|&k| k == Some(1)),
        "{pull:?}"
    );
    let push = posters("vllm_nixl_push");
    assert!(
        !push.is_empty() && push.iter().all(|&k| k == Some(0)),
        "{push:?}"
    );
    let by_hand = posters("pd_two_fabrics");
    assert!(
        !by_hand.is_empty() && by_hand.iter().all(Option::is_none),
        "{by_hand:?}"
    );
}
