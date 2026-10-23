//! Engines on devices (`docs/design/engine-device.md`): `device`, `engine …
//! on`, `pool … on`, `scheduler` and `worker` are parse-time sugar for the
//! kernel's step stage (`CStageKind::Step`), which no program writes any
//! more, so each test reads the step an engine lowers to from the compiled
//! `Program`; and every rule the design refuses does not link.

mod common;

use std::path::Path;

use serq::Overrides;
use serq::ir::{
    BinOp, CArg, CExpr, CIter, CRef, CServe, CStageKind, CStep, CtxVar, Fun, Program, Register,
    UnOp,
};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// `src` with the small engine's workload, `WORKLOAD` below.
fn compiled(src: &str, ov: &Overrides) -> Program {
    compiled_with(src, WORKLOAD, ov)
}

/// The deployment `src` with the workload's file `workload`.
fn compiled_with(src: &str, workload: &str, ov: &Overrides) -> Program {
    common::compile(&common::main_source(src), workload, ov)
        .unwrap_or_else(|e| panic!("{e}\n{src}\n{workload}"))
}

#[track_caller]
fn error(src: &str) -> String {
    error_with(src, WORKLOAD)
}

#[track_caller]
fn error_with(src: &str, workload: &str) -> String {
    match common::compile(&common::main_source(src), workload, &common::horizon(10.0)) {
        Ok(_) => panic!("links:\n{src}\n{workload}"),
        Err(e) => e,
    }
}

#[track_caller]
fn refused(src: &str, fragment: &str) {
    refused_with(src, WORKLOAD, fragment);
}

#[track_caller]
fn refused_with(src: &str, workload: &str, fragment: &str) {
    let e = error_with(src, workload);
    assert!(e.contains(fragment), "missing {fragment:?}:\n{e}");
}

/// The workload file an example is paired with (`examples/pairs.json`)
/// and the overrides that run it.
struct Paired {
    workload: String,
    ov: Overrides,
}

/// An example's text, and its workload with the overrides `ov`.
fn example(path: &Path, ov: &Overrides) -> (String, Paired) {
    let w = serq::paired_workload(path).unwrap_or_else(|| panic!("{}: no pair", path.display()));
    let paired = Paired {
        workload: std::fs::read_to_string(w).unwrap(),
        ov: ov.clone(),
    };
    (std::fs::read_to_string(path).unwrap(), paired)
}

/// An example's text `src`, perhaps edited, with its workload.
fn compile_paired(src: &str, paired: &Paired) -> Result<Program, String> {
    common::compile(src, &paired.workload, &paired.ov)
}

/// An example's text `src`, edited, refused with its workload.
#[track_caller]
fn refused_example(src: &str, ov: &Paired, fragment: &str) {
    match compile_paired(src, ov) {
        Ok(_) => panic!("links:\n{src}"),
        Err(e) => assert!(e.contains(fragment), "missing {fragment:?}:\n{e}"),
    }
}

/// An example compiled with its workload.
fn compiled_example(path: &Path, ov: &Overrides) -> Program {
    let (text, ov) = example(path, ov);
    compile_paired(&text, &ov).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn replaced(text: &str, pairs: &[(&str, &str)]) -> String {
    let mut t = text.to_string();
    for (a, b) in pairs {
        assert_eq!(t.matches(a).count(), 1, "{a}");
        t = t.replace(a, b);
    }
    t
}

/// The index of stage `name`, member `index` of an array or `None`.
fn stage(p: &Program, name: &str, index: Option<u32>) -> usize {
    p.stages
        .iter()
        .position(|s| s.name == name && s.index == index)
        .unwrap_or_else(|| panic!("no stage {name} {index:?}"))
}

/// The index of pool `name`, member `index` of an array or `None`.
fn pool(p: &Program, name: &str, index: Option<u32>) -> usize {
    p.pools
        .iter()
        .position(|q| q.name == name && q.index == index)
        .unwrap_or_else(|| panic!("no pool {name} {index:?}"))
}

/// The kernel step of stage `i`: what its engine lowered to.
fn step_at(p: &Program, i: usize) -> &CStep {
    match &p.stages[i].kind {
        CStageKind::Step(st) => st,
        _ => panic!("stage `{}` is no step stage", p.stages[i].name),
    }
}

/// The kernel step the single engine `name` lowered to.
fn step<'a>(p: &'a Program, name: &str) -> &'a CStep {
    step_at(p, stage(p, name, None))
}

/// What a step says of its schedule: its serve keys (`None` for the
/// exclusive-prefill rule), its per-run cap (0 for none) and its body
/// (`None` for vLLM's procedure).
#[derive(Debug, PartialEq)]
struct Schedule {
    keys: Option<Vec<CExpr>>,
    chunk: CExpr,
    body: Option<Vec<CIter>>,
}

impl Schedule {
    fn of(st: &CStep) -> Self {
        Schedule {
            keys: match &st.serve {
                CServe::By(keys) => Some(keys.clone()),
                CServe::ExclusivePrefill => None,
            },
            chunk: st.chunk.clone(),
            body: st.iteration.clone(),
        }
    }
}

/// vLLM's procedure: residents in admission order, then the waiting while
/// the iteration has not preempted; no cap, no body.
fn procedure() -> Schedule {
    Schedule {
        keys: Some(vec![]),
        chunk: num(0.0),
        body: None,
    }
}

fn num(x: f64) -> CExpr {
    CExpr::Num(x)
}

fn ctx(v: CtxVar) -> CExpr {
    CExpr::Ctx(v)
}

fn bin(op: BinOp, a: CExpr, b: CExpr) -> CExpr {
    CExpr::Binary(op, Box::new(a), Box::new(b))
}

fn not(a: CExpr) -> CExpr {
    CExpr::Unary(UnOp::Not, Box::new(a))
}

/// The kernel's `queued(q)`: what an engine's `waiting.count` reads.
fn queued(q: usize) -> CExpr {
    CExpr::Call(
        Fun::Queued,
        vec![CArg::Pool(CRef {
            base: q,
            count: 1,
            index: None,
        })],
    )
}

/// The body statements `serve;` and `admit;`, with no `only`, key or gate.
fn serve() -> CIter {
    CIter::Serve {
        only: None,
        by: None,
    }
}

fn admit() -> CIter {
    CIter::Admit {
        only: None,
        gate: None,
    }
}

/// The body that is the kernel's `serve only (p)` (#355): `serve only (p);
/// admit only (p) while (!preempted);`.
fn serve_only(p: CExpr) -> Vec<CIter> {
    vec![
        CIter::Serve {
            only: Some(p.clone()),
            by: None,
        },
        CIter::Admit {
            only: Some(p),
            gate: Some(not(ctx(CtxVar::Preempted))),
        },
    ]
}

/// `max(hbm(batch.kv_decode + batch.kv_prefill), compute(batch.tokens))`
/// with `hbm (k) = omega + beta * k` and `compute (t) = t * a`, the
/// examples' constants: the `worker` of the three engines below.
fn roofline() -> CExpr {
    CExpr::Call(
        Fun::Max,
        vec![
            CArg::Expr(bin(
                BinOp::Add,
                num(2e-4),
                bin(
                    BinOp::Mul,
                    num(2e-9),
                    bin(BinOp::Add, ctx(CtxVar::Kvb), ctx(CtxVar::Kvp)),
                ),
            )),
            CArg::Expr(bin(BinOp::Mul, ctx(CtxVar::Ntok), num(2e-5))),
        ],
    )
}

fn reg(r: usize) -> CExpr {
    CExpr::Reg(r)
}

/// `examples/multi-turn/vllm.sq` is vLLM's procedure on the kernel's step:
/// no body, residents in admission order, its memory `kv` admitted as soon
/// as it fits (`on gpu`) and `reqs` by the engine. `tokens cap B` is the
/// budget, `forward (c0 + …)` the cost, and the per-run cap the kernel's
/// `chunk`, applied only while more than one request is in the engine
/// (vLLM's `long_prefill_token_threshold`): `inf`, no cap, is its 0, and a cap of 512 is 512.
#[test]
fn vllm_as_an_engine_is_the_procedure() {
    let (text, ov) = example(
        &root().join("examples/multi-turn/vllm.sq"),
        &common::horizon(100.0),
    );
    let chunk = |cap: &str| {
        let src = replaced(
            &text,
            &[(
                r#"args.number("chunk_cap", inf)"#,
                &format!(r#"args.number("chunk_cap", {cap})"#),
            )],
        );
        let p = compile_paired(&src, &ov).unwrap_or_else(|e| panic!("{e}"));
        let st = step(&p, "vllm");
        assert_eq!(Schedule::of(st).keys, Some(vec![]));
        assert_eq!(st.iteration, None);
        assert_eq!(st.memory, Some(pool(&p, "kv", None)));
        assert_eq!(p.pools[pool(&p, "kv", None)].admit_via, None);
        let vllm = stage(&p, "vllm", None);
        let reqs = pool(&p, "reqs", None);
        assert_eq!(p.pools[reqs].admit_via, Some(vllm));
        assert_eq!(st.budget, num(8192.0));
        assert_eq!(st.cost, bin(BinOp::Add, num(0.0), roofline()));
        // `running.count + waiting.count > 1 ? cap : inf`
        let CExpr::Cond(more, cap, none) = &st.chunk else {
            panic!("{:?}", st.chunk)
        };
        assert_eq!(
            **more,
            bin(
                BinOp::Gt,
                bin(BinOp::Add, ctx(CtxVar::Nres), queued(reqs)),
                num(1.0)
            )
        );
        assert_eq!(**none, num(0.0));
        (**cap).clone()
    };
    assert_eq!(chunk("inf"), num(0.0));
    assert_eq!(chunk("512"), num(512.0));
}

/// SGLang's schedule and TGI's are bodies, which the kernel keeps as
/// bodies. TGI's device pool is its memory and admitted by its engine
/// (`pool kv on tgi.gpu`); SGLang's memory is admitted as soon as it fits,
/// and its `waiting.count` reads `reqs`, the one queue its engine admits.
#[test]
fn sglang_and_tgi_as_engines_are_bodies() {
    let base = root().join("examples/engines");
    let ov = common::horizon(100.0);
    let tgi = compiled_example(&base.join("tgi.sq"), &ov);
    let st = step(&tgi, "tgi");
    let e = stage(&tgi, "tgi", None);
    // `state just = 0;`
    let just = Register {
        name: "just".into(),
        stage: e,
        init: 0.0,
    };
    assert_eq!(tgi.registers, vec![just]);
    assert_eq!((&st.budget, &st.cost), (&num(4096.0), &roofline()));
    // `advance running; if (just == 0 || running.count == 0) { admit
    // waiting taking …; } set just = waiting.admitted > 0;`
    assert_eq!(
        st.iteration,
        Some(vec![
            serve(),
            CIter::Branch(
                bin(
                    BinOp::Or,
                    bin(BinOp::Eq, reg(0), num(0.0)),
                    bin(BinOp::Eq, ctx(CtxVar::Nres), num(0.0)),
                ),
                vec![admit()],
                vec![],
            ),
            CIter::Set(0, bin(BinOp::Gt, ctx(CtxVar::Admitted), num(0.0))),
        ])
    );
    let kv = pool(&tgi, "kv", None);
    assert_eq!(st.memory, Some(kv));
    assert_eq!(tgi.pools[kv].admit_via, Some(e));

    let sglang = compiled_example(&base.join("sglang.sq"), &ov);
    let st = step(&sglang, "sglang");
    let e = stage(&sglang, "sglang", None);
    let (kv, reqs) = (pool(&sglang, "kv", None), pool(&sglang, "reqs", None));
    // the program's `r0`, `r_min`, `r_decay`, folded as it folds them
    let r0 = 0.7;
    let r_min = r0 * 0.14;
    let r_decay = (r0 - r_min) / 600.0;
    // `state ratio = r0; state backlog = 0;`
    let state = |name: &str, init| Register {
        name: name.into(),
        stage: e,
        init,
    };
    assert_eq!(
        sglang.registers,
        vec![state("ratio", r0), state("backlog", 0.0)]
    );
    let (ratio, backlog) = (0, 1);
    let max = |a, b| CExpr::Call(Fun::Max, vec![CArg::Expr(a), CArg::Expr(b)]);
    let min = |a, b| CExpr::Call(Fun::Min, vec![CArg::Expr(a), CArg::Expr(b)]);
    assert_eq!((&st.budget, &st.cost), (&num(8192.0), &roofline()));
    assert_eq!(
        st.iteration,
        Some(vec![
            // if (running.count == 0 && backlog == 0) { set ratio = r0; }
            CIter::Branch(
                bin(
                    BinOp::And,
                    bin(BinOp::Eq, ctx(CtxVar::Nres), num(0.0)),
                    bin(BinOp::Eq, reg(backlog), num(0.0)),
                ),
                vec![CIter::Set(ratio, num(r0))],
                vec![],
            ),
            // advance running only (!decoding);
            CIter::Serve {
                only: Some(not(ctx(CtxVar::Decoding))),
                by: None,
            },
            // admit waiting;
            admit(),
            // if (batch.tokens == 0) { advance running; if … }
            CIter::Branch(
                bin(BinOp::Eq, ctx(CtxVar::Ntok), num(0.0)),
                vec![
                    serve(),
                    CIter::Branch(
                        bin(BinOp::Gt, ctx(CtxVar::Preempted), num(0.0)),
                        // set ratio = max(r_min, min(1, retract_steps / M));
                        vec![CIter::Set(
                            ratio,
                            max(
                                num(r_min),
                                min(num(1.0), bin(BinOp::Div, num(20.0), num(2048.0))),
                            ),
                        )],
                        // set ratio = max(r_min, ratio - r_decay);
                        vec![CIter::Set(
                            ratio,
                            max(num(r_min), bin(BinOp::Sub, reg(ratio), num(r_decay))),
                        )],
                    ),
                ],
                vec![],
            ),
            // set backlog = waiting.count > 0;
            CIter::Set(backlog, bin(BinOp::Gt, queued(reqs), num(0.0))),
        ])
    );
    assert_eq!(st.memory, Some(kv));
    assert_eq!(sglang.pools[kv].admit_via, None);
    assert_eq!(sglang.pools[reqs].admit_via, Some(e));
}

/// A small engine, for the forms below: its `reqs` admitted by it, its
/// memory `kv` on its device.
const WORKLOAD: &str = "workload { arrive batch(3); init { set prompt = 2; } send (prompt); }\n";
const SERVER: &str = "
server (prompt) { vllm (prompt, 3); }
";

/// `scheduler` with the request's admission on each `admit waiting`: a slot
/// of `reqs` and the prompt's blocks of `kv`, which its KV grows.
fn taking(schedule: &str) -> String {
    let mut out = String::new();
    let mut rest = schedule;
    while let Some(i) = rest.find("admit waiting") {
        let end = rest[i..].find(';').map_or(rest.len(), |e| i + e);
        out.push_str(&rest[..end]);
        out.push_str(" taking reqs (1), kv (request.input)");
        rest = &rest[end..];
    }
    out + rest
}

/// vLLM's engine on one device, its request a prefill and two decodes (the
/// call's output 3, the prefill's token and two). A schedule the refusals
/// write without `admit waiting` takes nothing.
fn engine(schedule: &str) -> String {
    let schedule = taking(schedule);
    format!(
        "device gpu {{ step_time (t) = 1; kv cap 100; }}
engine vllm on gpu {{
  reqs cap 8;
  tokens cap 8;
  scheduler {{ {schedule} }}
  worker {{ forward (step_time(batch.tokens)); }}
}}
pool reqs on vllm {{ }}
pool kv on gpu {{ preempt lifo; }}
{SERVER}"
    )
}

/// The step `engine(schedule)` lowers to.
fn lowered(schedule: &str) -> Schedule {
    let p = compiled(&engine(schedule), &common::horizon(20.0));
    Schedule::of(step(&p, "vllm"))
}

/// The block is named for the component it is, the engine's scheduler
/// (#555): the old word is refused with the new one.
#[test]
fn schedule_is_written_scheduler() {
    let src = engine("advance running; admit waiting while (running.preempted == 0);")
        .replace("scheduler {", "schedule {");
    refused(&src, "an engine writes this as `scheduler { … }`");
}

/// The batch's time is the worker's forward (#555): `execute (T);` is
/// refused with it, and a worker has one forward and nothing else yet.
#[test]
fn execute_is_the_workers_forward() {
    let ok = engine("advance running; admit waiting while (running.preempted == 0);");
    let forward = "worker { forward (step_time(batch.tokens)); }";
    refused(
        &ok.replace(forward, "execute (step_time(batch.tokens));"),
        "an engine writes this as `worker { forward (T); }`",
    );
    refused(
        &ok.replace(forward, "worker { }"),
        "a worker needs `forward (T);`",
    );
    refused(
        &ok.replace(
            forward,
            "worker { forward (1); forward (step_time(batch.tokens)); }",
        ),
        "`forward` twice",
    );
    refused(
        &ok.replace(forward, "worker { set x = 1; }"),
        "a worker takes `forward (T);`",
    );
    refused(
        &ok.replace(forward, "worker { forward (running.count); }"),
        "counts the residents",
    );
}

/// Each form of the kernel's step is a schedule: the procedure, spelt
/// `running.preempted == 0`, is no body, an order is the serve keys, a
/// common `only` is the body `serve only` is, `exclusive prefill` is its
/// rule and a per-run cap the `chunk`; any other schedule is its body, as
/// written, a gate that computes the procedure's otherwise among them
/// (#721).
#[test]
fn the_stage_forms_are_schedules() {
    let p = compiled(
        &engine("advance running; admit waiting while (running.preempted == 0);"),
        &common::horizon(20.0),
    );
    // the engine's clauses: `tokens cap` is the budget, `worker` the cost,
    // the device's pool the memory, admitted as soon as it fits, and the
    // engine's `reqs` cap a pool it admits
    let st = step(&p, "vllm");
    let (kv, reqs) = (pool(&p, "kv", None), pool(&p, "reqs", None));
    assert_eq!((&st.budget, &st.cost), (&num(8.0), &num(1.0)));
    assert_eq!(st.memory, Some(kv));
    assert_eq!((p.pools[kv].cap, p.pools[kv].admit_via), (100.0, None));
    assert_eq!(
        (p.pools[reqs].cap, p.pools[reqs].admit_via),
        (8.0, Some(stage(&p, "vllm", None)))
    );
    let decoding = || ctx(CtxVar::Decoding);
    let decode_first = || CExpr::Cond(Box::new(decoding()), Box::new(num(0.0)), Box::new(num(1.0)));
    // the body `advance running; admit waiting while (g);`, which the
    // procedure is only when `g` is spelt `running.preempted == 0`
    let gated = |g: CExpr| Schedule {
        body: Some(vec![
            serve(),
            CIter::Admit {
                only: None,
                gate: Some(g),
            },
        ]),
        ..procedure()
    };
    for (schedule, kernel) in [
        (
            "advance running; admit waiting while (running.preempted == 0);",
            procedure(),
        ),
        (
            "advance running; admit waiting while (!running.preempted);",
            gated(not(ctx(CtxVar::Preempted))),
        ),
        (
            "advance running; admit waiting while (0 == running.preempted);",
            gated(bin(BinOp::Eq, num(0.0), ctx(CtxVar::Preempted))),
        ),
        (
            "advance running; admit waiting while (running.preempted < 1);",
            gated(bin(BinOp::Lt, ctx(CtxVar::Preempted), num(1.0))),
        ),
        // an order in a body is its `serve`'s, and the step's own keys none
        (
            "advance running decode first; admit waiting while (!running.preempted);",
            Schedule {
                body: Some(vec![
                    CIter::Serve {
                        only: None,
                        by: Some(vec![decode_first()]),
                    },
                    CIter::Admit {
                        only: None,
                        gate: Some(not(ctx(CtxVar::Preempted))),
                    },
                ]),
                ..procedure()
            },
        ),
        (
            "advance running decode first; admit waiting while (running.preempted == 0);",
            Schedule {
                keys: Some(vec![decode_first()]),
                ..procedure()
            },
        ),
        (
            "advance running by (remaining); admit waiting while (running.preempted == 0);",
            Schedule {
                keys: Some(vec![ctx(CtxVar::Remaining)]),
                ..procedure()
            },
        ),
        (
            "advance running only (decoding); admit waiting only (decoding) while (running.preempted == 0);",
            Schedule {
                body: Some(serve_only(decoding())),
                ..procedure()
            },
        ),
        (
            "exclusive prefill; admit waiting while (running.preempted == 0);",
            Schedule {
                keys: None,
                ..procedure()
            },
        ),
        (
            "advance running each at most (4); admit waiting while (running.preempted == 0) each at most (4);",
            Schedule {
                chunk: num(4.0),
                ..procedure()
            },
        ),
        (
            "advance running; if (running.count == 0) { admit waiting; }",
            Schedule {
                body: Some(vec![
                    serve(),
                    CIter::Branch(
                        bin(BinOp::Eq, ctx(CtxVar::Nres), num(0.0)),
                        vec![admit()],
                        vec![],
                    ),
                ]),
                ..procedure()
            },
        ),
        (
            "admit waiting; advance running;",
            Schedule {
                body: Some(vec![admit(), serve()]),
                ..procedure()
            },
        ),
    ] {
        assert_eq!(lowered(schedule), kernel, "{schedule}");
    }
}

/// An engine names each value one way in all its clauses: `running.…` the
/// residents, `waiting.…` the queues, `batch.…` the batch, and none of them
/// reads the kernel's name for it (#411). The kernel's `decoders` and
/// `kv_decode` are the residents' before the batch is formed and the
/// batch's in `cost`, which a budget or an `only` may leave short, so an
/// engine reads `running.decoding` in `tokens cap` and `scheduler` and
/// `batch.decoding` in `worker` (#416).
#[test]
fn an_engine_reads_its_lists_in_every_clause() {
    let ov = common::horizon(20.0);
    let ok = "advance running; admit waiting while (running.preempted == 0);";
    let reqs = pool(&compiled(&engine(ok), &ov), "reqs", None);
    // each value as the kernel's context variable it reads: in `tokens
    // cap 8 + v`, the budget `8 + v`; in `forward (step_time(…) + v)`, the
    // cost `1 + v`
    let tokens_cap = |v: &str| {
        let p = compiled(
            &replaced(
                &engine(ok),
                &[("tokens cap 8;", &format!("tokens cap 8 + {v};"))],
            ),
            &ov,
        );
        let CExpr::Binary(BinOp::Add, _, e) = &step(&p, "vllm").budget else {
            panic!("{v}")
        };
        (**e).clone()
    };
    let execute = |v: &str| {
        let p = compiled(
            &replaced(
                &engine(ok),
                &[(
                    "worker { forward (step_time(batch.tokens)); }",
                    &format!("worker {{ forward (step_time(batch.tokens) + {v}); }}"),
                )],
            ),
            &ov,
        );
        let CExpr::Binary(BinOp::Add, _, e) = &step(&p, "vllm").cost else {
            panic!("{v}")
        };
        (**e).clone()
    };
    for (v, kernel) in [
        ("running.decoding", ctx(CtxVar::Ndec)),
        ("running.count", ctx(CtxVar::Nres)),
        ("running.kv_decode", ctx(CtxVar::Kvb)),
        ("waiting.count", queued(reqs)),
    ] {
        assert_eq!(tokens_cap(v), kernel, "tokens cap {v}");
    }
    for (v, kernel) in [
        ("batch.tokens", ctx(CtxVar::Ntok)),
        ("batch.decoding", ctx(CtxVar::Ndec)),
        ("batch.kv_decode", ctx(CtxVar::Kvb)),
        ("batch.prefilled", ctx(CtxVar::Npre)),
        ("batch.attention", ctx(CtxVar::Attn)),
    ] {
        assert_eq!(execute(v), kernel, "forward {v}");
    }
    for (from, to, why) in [
        (
            "tokens cap 8;",
            "tokens cap 8 - residents;",
            "`residents` is `running.count`",
        ),
        (
            "worker { forward (step_time(batch.tokens)); }",
            "worker { forward (step_time(batch.tokens) + max(residents, 1)); }",
            "`residents` is `running.count`",
        ),
        (
            "tokens cap 8;",
            "tokens cap max(decoders, 1);",
            "`decoders` is `running.decoding`",
        ),
        (
            "worker { forward (step_time(batch.tokens)); }",
            "worker { forward (step_time(batch.tokens) + running.decoding); }",
            "counts the residents, in the batch or not, and the batch's is `batch.decoding`",
        ),
        (
            "worker { forward (step_time(batch.tokens)); }",
            "worker { forward (step_time(batch.tokens) + kv_decode); }",
            "`kv_decode` is `batch.kv_decode` here",
        ),
        (
            "worker { forward (step_time(batch.tokens)); }",
            "worker { forward (step_time(batch.tokens) + tokens); }",
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
            "`batch.decoding` is the formed batch's, read in `worker`",
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
            "read in its `if`, `while` and `set`, not in `only`",
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
            "read in its `if`, `while` and `set`, not in `tokens cap`",
        ),
        (
            "worker { forward (step_time(batch.tokens)); }",
            "worker { forward (step_time(batch.tokens) + waiting.admitted); }",
            "read in its `if`, `while` and `set`, not in `worker`",
        ),
        (
            "worker { forward (step_time(batch.tokens)); }",
            "worker { forward (step_time(batch.tokens) + preempted); }",
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
    let tokens_below_8 = || bin(BinOp::Lt, ctx(CtxVar::Ntok), num(8.0));
    assert_eq!(
        lowered("advance running; if (batch.tokens < 8) { admit waiting; }").body,
        Some(vec![
            serve(),
            CIter::Branch(tokens_below_8(), vec![admit()], vec![])
        ])
    );
    // and after an `only`, read for each resident, the statements read it again
    assert_eq!(
        lowered(
            "advance running only (running.count > 0); if (batch.tokens < 8) { admit \
             waiting; }"
        )
        .body,
        Some(vec![
            CIter::Serve {
                only: Some(bin(BinOp::Gt, ctx(CtxVar::Nres), num(0.0))),
                by: None,
            },
            CIter::Branch(tokens_below_8(), vec![admit()], vec![])
        ])
    );
    // the bug as found: a list value as a call argument, in a schedule
    assert_eq!(
        lowered("advance running; admit waiting while (max(running.decoding, 1) == 1);").body,
        Some(vec![
            serve(),
            CIter::Admit {
                only: None,
                gate: Some(bin(
                    BinOp::Eq,
                    CExpr::Call(
                        Fun::Max,
                        vec![CArg::Expr(ctx(CtxVar::Ndec)), CArg::Expr(num(1.0))]
                    ),
                    num(1.0)
                )),
            }
        ])
    );
    // outside an engine there are no lists to name
    refused(
        &format!("stage p : ps(1 + running.count);\n{}", engine(ok)),
        "is a value of an engine",
    );
}

/// A predicate `advance running` and `admit waiting` share is named with a
/// `def`, read where each `only` stands, and the schedule is still vLLM's
/// procedure under it: the body the kernel's `serve only (p)` is (#412).
#[test]
fn a_def_names_the_predicate_advance_and_admit_share() {
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
    let decoding = || ctx(CtxVar::Decoding);
    let kernel = Schedule {
        body: Some(serve_only(CExpr::Cond(
            Box::new(bin(BinOp::Gt, ctx(CtxVar::Ndec), num(0.0))),
            Box::new(decoding()),
            Box::new(not(decoding())),
        ))),
        ..procedure()
    };
    let ov = common::horizon(20.0);
    for src in [named, mixed] {
        assert_eq!(Schedule::of(step(&compiled(&src, &ov), "vllm")), kernel);
    }
}

/// An aggregate's index may not take a register's name, as it may take
/// neither an attribute's nor a constant's: the sum read the index where
/// the register was meant, `sum r in 2 (r)` as `0 + 1` (#558). Whatever the
/// order the names are declared in: a register's initial value or a `let`
/// is folded before a later register is taken (#558 review).
#[test]
fn an_aggregate_index_is_no_register() {
    let refusal = "is also an engine's register";
    let src = engine(
        "set r = (sum r in 2 (r)) * 0; advance running; \
         admit waiting while (running.preempted == 0);",
    )
    .replace("tokens cap 8;", "tokens cap 8; state r = 0;");
    refused(&src, &format!("`sum r in`: `r` {refusal}"));
    let ok = "set q = q; set s = s; advance running; admit waiting while (running.preempted == 0);";
    for state in [
        "state q = 0; state s = sum q in 2 (q);",
        "state s = sum q in 2 (q); state q = 0;",
    ] {
        let src = engine(ok).replace("tokens cap 8;", &format!("tokens cap 8; {state}"));
        refused(&src, &format!("`sum q in`: `q` {refusal}"));
    }
    let src = format!(
        "let n = sum q in 2 (q);\n{}",
        engine(ok).replace("tokens cap 8;", "tokens cap 8; state q = 0; state s = n;")
    );
    refused(&src, &format!("`sum q in`: `q` {refusal}"));
    // the bug as found: an array size is folded by the parser, which knows
    // no register, and reached the linker as a number, so it linked (#561)
    let with_r = |size: &str| {
        format!(
            "stage colo[{size}] : delay;\n{}",
            engine("set r = r; advance running; admit waiting while (running.preempted == 0);")
                .replace("tokens cap 8;", "tokens cap 8; state r = 0;")
                .replace(
                    "vllm (prompt, 3);",
                    "run colo[0] (cost(colo, 1)); vllm (prompt, 3);"
                )
        )
    };
    refused(
        &with_r("sum r in 2 (1)"),
        &format!("`sum r in`: `r` {refusal}"),
    );
    compiled(&with_r("sum k in 2 (1)"), &common::horizon(20.0));
    // a branch a constant condition does not take is not evaluated, but it
    // is the program's: in a constant the linker folds and in a cap it
    // chooses, as in the size the parser folds (#561 review)
    let ok = with_r("2");
    let untaken = |j: &str| {
        let cap = format!("each at most (1 ? 4 : sum {j} in 2 (1))");
        [
            ok.replace(
                "kv cap 100;",
                &format!("kv cap 1 ? 100 : sum {j} in 2 (0);"),
            ),
            ok.replace("advance running;", &format!("advance running {cap};"))
                .replace(
                    "(running.preempted == 0)",
                    &format!("(running.preempted == 0) {cap}"),
                ),
        ]
    };
    for src in untaken("r") {
        refused(&src, &format!("`sum r in`: `r` {refusal}"));
    }
    for src in untaken("k") {
        compiled(&src, &common::horizon(20.0));
    }
}

/// A register no `set` writes keeps its first value, a `let` spelt a second
/// way. The linker refused one for vLLM's procedure alone, so
/// `examples/multi-turn/vllm.sq` with `state k = 0;` linked once its gate
/// was spelt `!running.preempted`, `0 == running.preempted` or
/// `running.preempted < 1`, each a body (#746). Every spelling is refused
/// now, and a `set` in an `if` sets it.
#[test]
fn a_register_no_schedule_sets_does_not_link() {
    let (src, ov) = example(
        &root().join("examples/multi-turn/vllm.sq"),
        &common::horizon(100.0),
    );
    let src = replaced(
        &src,
        &[("tokens cap B;", "tokens cap B;\n    state k = 0;")],
    );
    for gate in [
        "running.preempted == 0",
        "!running.preempted",
        "0 == running.preempted",
        "running.preempted < 1",
    ] {
        let prog = replaced(
            &src,
            &[("while (running.preempted == 0)", &format!("while ({gate})"))],
        );
        refused_example(
            &prog,
            &ov,
            "engine `vllm`: register `k` is set by nothing: a schedule that sets it is \
             written with `set k = …;`",
        );
    }
    let prog = replaced(
        &src,
        &[(
            "advance running each at most (threshold);",
            "advance running each at most (threshold);\n      \
             if (waiting.count > 0) { set k = k + 1; }",
        )],
    );
    compile_paired(&prog, &ov).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn the_design_refuses_what_it_says() {
    let ok = "advance running; admit waiting while (running.preempted == 0);";
    // vLLM's procedure, with or without a shared `only`, sets no register,
    // so one it declares would keep its first value: one is set with `set`
    for schedule in [
        ok,
        "advance running only (decoding); \
         admit waiting only (decoding) while (running.preempted == 0);",
    ] {
        refused(
            &engine(schedule).replace("tokens cap 8;", "tokens cap 8; state k = 0;"),
            "engine `vllm`: register `k` is set by nothing: a schedule that sets it is \
             written with `set k = …;`",
        );
    }
    // `step_time (t) = 1` drops its argument, whose names are still resolved
    // (#431)
    refused(
        &engine(ok).replace("step_time(batch.tokens)", "step_time(no_such_name)"),
        "unknown name `no_such_name`",
    );
    // a device's time resource is read in an engine's `worker` only
    refused(
        &engine(ok).replace("tokens cap 8;", "tokens cap step_time(1);"),
        "read in an engine's `worker`",
    );
    // a pool on a device or an engine takes its cap and its admitter from it
    refused(
        &engine(ok).replace(
            "pool kv on gpu { preempt lifo; }",
            "pool kv on gpu { cap 5; }",
        ),
        "takes its capacity from it",
    );
    refused(
        &engine(ok).replace(
            "pool reqs on vllm { }",
            "pool reqs on vllm { admit via vllm; }",
        ),
        "who admits a pool is said in its `on`",
    );
    refused(
        &engine(ok).replace("pool kv on gpu { preempt lifo; }", "pool kv[2] on gpu { }"),
        "write no `[N]`",
    );
    refused(
        &engine(ok).replace("pool reqs on vllm { }", "pool slots on vllm { }"),
        "error[E-pool-no-capacity]: `vllm` has no capacity `slots`",
    );
    refused(
        &engine(ok).replace("pool reqs on vllm { }", "pool slots on vllm { }"),
        "help: a pool on `vllm` is one of its capacities: declare `slots cap …;` in it",
    );
    // a capacity of a nearby name is the one likely meant (#645 A4)
    refused(
        &engine(ok).replace("pool reqs on vllm { }", "pool req on vllm { }"),
        "help: did you mean `reqs`?",
    );
    // on the device an engine runs on, a second capacity would be a second
    // pool there (E-device-two-pools): the way out is that one's, a pool on
    // no device, whichever order the two pools are written in, and no
    // capacity of a nearby name is offered
    for two in [
        "pool kv on gpu { preempt lifo; } pool kv2 on gpu { }",
        "pool kv2 on gpu { } pool kv on gpu { preempt lifo; }",
    ] {
        let e = error(&engine(ok).replace("pool kv on gpu { preempt lifo; }", two));
        assert!(e.contains("error[E-pool-no-capacity]"), "{e}");
        assert!(
            e.contains("help: the device an engine runs on holds one pool, its KV: declare `kv2` on no device"),
            "{e}"
        );
        assert!(!e.contains("did you mean") && !e.contains("kv2 cap"), "{e}");
    }
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
        &engine(ok).replace("worker { forward (step_time(batch.tokens)); }", ""),
        "needs `worker { forward (T); }`",
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
        &engine("let c = 4; advance running; if (c > 1) { admit waiting; }"),
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
        lowered(
            "let c = 0; let t = c > 0 ? c : inf; advance running each at most (t); \
             admit waiting while (running.preempted == 0) each at most (t);"
        ),
        procedure()
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
    // a computed cap is `max(k, e)` above a positive constant `k` (#442)
    refused(
        &engine("advance running each at most (running.count);"),
        "`max(k, e)` with `k` a constant above 0",
    );
    refused(
        &engine("advance running each at most (min(4, running.count));"),
        "a cap that shrinks writes its floor: `max(1, min(k, e))`",
    );
    refused(
        &engine("advance running each at most (max(0, running.count));"),
        "`each at most (max(0, …))`: a run could be given 0",
    );
    refused(
        &engine("let k = 2 - 3; advance running each at most (max(running.count, k));"),
        "`each at most (max(-1, …))`: a run could be given -1",
    );
    refused(
        &engine("let k = inf; advance running each at most (max(k, running.count));"),
        "`inf` inside an `each at most`'s arithmetic",
    );
    refused(
        &format!(
            "let k = inf;\n{}",
            engine("advance running each at most (max(k, running.count));")
        ),
        "is no cap whatever it reads; write `inf`",
    );
    refused(
        &engine("advance running each at most (max(running.count, waiting.count));"),
        "`max(k, e)` with `k` a constant above 0",
    );
    refused(
        &engine("advance running each at most (2 * max(2, running.count));"),
        "`max(k, e)` with `k` a constant above 0",
    );
    assert_eq!(
        lowered(
            "let k = 2; let t = max(k, floor(8 / running.count)); \
             advance running each at most (t); \
             admit waiting while (running.preempted == 0) each at most (t);"
        )
        .chunk,
        CExpr::Call(
            Fun::Max,
            vec![
                CArg::Expr(num(2.0)),
                CArg::Expr(CExpr::Call(
                    Fun::Floor,
                    vec![CArg::Expr(bin(BinOp::Div, num(8.0), ctx(CtxVar::Nres)))]
                )),
            ]
        )
    );
    // a `let` reads the ones above it
    assert_eq!(
        lowered(
            "let a = 4; let b = a + 1; advance running each at most (b); \
             admit waiting while (running.preempted == 0) each at most (b);"
        )
        .chunk,
        num(5.0)
    );
    // one device runs one engine, with pools on it or none
    refused(
        &format!(
            "device gpu {{ t (x) = 1; }}
engine e1 on gpu {{ tokens cap 4; scheduler {{ advance running; }} worker {{ forward (t(batch.tokens)); }} }}
engine e2 on gpu {{ tokens cap 4; scheduler {{ advance running; }} worker {{ forward (t(batch.tokens)); }} }}
{SERVER}"
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
        "a queue named `waiting` would make the one word mean two things",
    );
    refused(
        &engine("advance running; if (residents == 0) { admit waiting; }"),
        "`running.count`",
    );
    refused(
        &engine("advance running; admit waiting while (!preempted);"),
        "`running.preempted`",
    );
    refused(&engine("serve; admit waiting;"), "`advance running`");
    refused(&engine("advance running; admit;"), "`admit waiting;`");
    refused(
        &engine("if (running.size > 0) { advance running; }"),
        "`running` has `count`",
    );
    let gated = ", its gate spelt `running.preempted == 0`: a gate written otherwise makes \
                 the schedule a body";
    let e = error(&engine(
        "exclusive prefill; if (running.count == 0) { admit waiting; }",
    ));
    assert!(
        e.contains("takes back decodes already chosen") && !e.contains(gated),
        "{e}"
    );
    // the exclusive rule's schedule has the procedure's one spelling: a gate
    // written otherwise makes it a body, which cannot take decodes back
    // (#721), and the refusal names the spelling and says the gate differs
    let e = error(&engine(
        "exclusive prefill; admit waiting while (!running.preempted);",
    ));
    assert!(
        e.contains("takes back decodes already chosen")
            && e.contains(&format!(
                "`exclusive prefill; admit waiting while (running.preempted == 0);`{gated}"
            )),
        "{e}"
    );
    // one engine per device, and a family of them a queue family (#539)
    refused(
        &engine(ok).replace("engine vllm on gpu", "engine vllm[2] on gpu"),
        "engine family `vllm[2]`: a family of engines is a queue family",
    );
    refused(
        &engine(ok).replace("device gpu {", "device gpu[2] {"),
        "an engine is one per device, and `gpu` is a family of 2",
    );
    refused(
        &engine(ok).replace("on gpu {\n  reqs", "on cpu {\n  reqs"),
        "no device `cpu`",
    );
    // a pool on a name nothing declares is pointed at that name, the owner
    // (line 9, `pool kv on gpuu`, column 12), not the pool's
    let e = error(&engine(ok).replace("pool kv on gpu", "pool kv on gpuu"));
    assert!(
        e.starts_with("deployment.sq: 9:12: no device or engine `gpuu`"),
        "{e}"
    );
    assert!(e.contains("did you mean `gpu`?"), "{e}");
    // a context variable's old name is answered with the engine's name
    let e = error(&engine(ok).replace("forward (step_time(batch.tokens))", "forward (ntok)"));
    assert!(
        e.contains("in an engine, `ntok` is `batch.tokens` here"),
        "{e}"
    );
}

/// A name the program declares is the program's in an engine too, though a
/// context variable once had it: `let attn` is not `attention` renamed.
#[test]
fn an_engine_reads_the_programs_own_names() {
    let with = |c: &str| {
        engine("advance running; admit waiting;").replace(
            "forward (step_time(batch.tokens))",
            &format!("forward (step_time(batch.tokens) + {c} * batch.attention)"),
        )
    };
    let ov = common::horizon(10.0);
    assert_eq!(
        step(
            &compiled(&format!("let attn = 1e-3;\n{}", with("attn")), &ov),
            "vllm"
        )
        .cost,
        step(
            &compiled(&format!("let attn = 1e-3;\n{}", with("1e-3")), &ov),
            "vllm"
        )
        .cost,
    );
}

/// A family of engines is a queue family (#539): `engine E[N]` is refused
/// with the queue to write. In the queue family each member's engine reads
/// its own device's `kv` as its memory and admits it (`on vllm.gpu`), or
/// admits nothing (`on gpu`); a member's schedule cannot count every
/// member's queues.
#[test]
fn a_family_of_engines_is_a_queue_family() {
    refused_with(
        "device gpu[2] { kv cap 100; } pool kv on gpu { }
engine vllm[2] on gpu { tokens cap 8; scheduler { advance running; admit waiting while (running.preempted == 0); } worker { forward (1); } }",
        "workload { arrive batch(4); init { set prompt = 2; } }",
        "engine family `vllm[2]`: a family of engines is a queue family, `queue vllm[2] : ROLE",
    );
    let ov = common::horizon(20.0);
    let queue = |schedule: &str, on: &str| {
        format!(
            "queue vllm[2] : agg {{
  device gpu {{ kv cap 100; }}
  engine on gpu {{ tokens cap 8; scheduler {{ {schedule} }} worker {{ forward (1); }} }}
  pool kv on {on} {{ }}
  decode (prompt) {{ hold kv (cost(kv, prompt)) {{ run vllm prefill (cost(vllm, prompt)) growing kv; }} }}
}}
server (prompt, j) {{ vllm[j].decode (prompt); }}"
        )
    };
    let w = "workload { arrive batch(4); init { set prompt = 2; set j = serial < 2 ? 0 : 1; } send (prompt, j); }";
    let ok = "advance running; admit waiting while (running.preempted == 0);";
    for (on, admitted) in [("vllm.gpu", true), ("gpu", false)] {
        let src = queue(ok, on);
        let p = compiled_with(&src, w, &ov);
        for i in [0, 1] {
            let (e, kv) = (stage(&p, "vllm", Some(i)), pool(&p, "vllm.kv", Some(i)));
            assert_eq!(step_at(&p, e).memory, Some(kv), "{src}");
            assert_eq!(p.pools[kv].cap, 100.0);
            assert_eq!(p.pools[kv].admit_via, admitted.then_some(e), "{src}");
        }
    }
    refused_with(
        &queue(
            "let c = waiting.count > 0 ? 2 : inf; advance running each at most (c); \
             admit waiting while (running.preempted == 0) each at most (c);",
            "vllm.gpu",
        ),
        w,
        "is a family, and its `waiting.count`",
    );
    // nor its `tokens cap`
    refused_with(
        &queue(ok, "vllm.gpu").replace("tokens cap 8;", "tokens cap 8 + waiting.count;"),
        w,
        "which no member's engine may",
    );
}

/// `on ENGINE.DEVICE` names the engine on the device, declared above the
/// pool: that engine admits the pool's queue.
#[test]
fn a_pool_names_the_engine_that_admits_it() {
    let ok = engine("advance running; admit waiting while (running.preempted == 0);");
    let ov = common::horizon(20.0);
    let p = compiled(&ok.replace("pool kv on gpu", "pool kv on vllm.gpu"), &ov);
    assert_eq!(
        p.pools[pool(&p, "kv", None)].admit_via,
        Some(stage(&p, "vllm", None))
    );
    refused(
        &ok.replace("pool kv on gpu", "pool kv on sglang.gpu"),
        "no engine `sglang`",
    );
    refused(
        &ok.replace("device gpu {", "device npu { kv cap 1; }\ndevice gpu {")
            .replace("pool kv on gpu", "pool kv on vllm.npu"),
        "`vllm` runs on `gpu`, not `npu`",
    );
    refused(
        &ok.replace("pool kv on gpu", "pool kv on vllm.vllm"),
        "`vllm` is not a device",
    );
    // the kernel's option is said in `on`, and the error says how
    refused(
        &ok.replace("pool kv on gpu { ", "pool kv on gpu { admit via vllm; "),
        "`pool NAME on E.DEVICE` for the device's",
    );
}

/// A family of capacities in one engine (`reqs[2] cap 4;`) would be N
/// queues it admits, and the request's `taking` names one pool: it is
/// refused, with the capacity and the queue family to write instead. The
/// text below is the one the later tests start from.
const QUEUES: &str = "device gpu { kv cap 1000; }
engine llm on gpu {
  reqs[2] cap 4;
  tokens cap 64 + waiting.count;
  scheduler { advance running; admit waiting while (running.preempted == 0); }
  worker { forward (1); }
}
pool kv on gpu { }
pool reqs on llm { }
server (n, t) { hold reqs[t] (cost(reqs[t], 1)), kv (cost(kv, n)) { run llm prefill (cost(llm, n)) growing kv; } }";
const QUEUES_WORKLOAD: &str = "workload { arrive poisson(1); init { set n = 10; set t = ~bernoulli(0.5); } send (n, t); session { turn; end; } }";

#[test]
fn an_engine_admits_no_family_of_capacities() {
    refused_with(
        QUEUES,
        QUEUES_WORKLOAD,
        "`reqs[2]` in engine `llm`: an engine admits through its request's `taking`, which names \
         one `reqs`; write `reqs cap c;`, or a queue family `queue Q[2]` for 2 engines",
    );
}

/// In a queue an entry names its own pool without an index (`reqs`, not
/// `reqs[i]`), so a capacity family in a queue's engine could be declared
/// and never held: it is refused where it is declared.
#[test]
fn a_queues_engine_holds_no_family_capacity() {
    refused_with(
        "queue Q : prefill {
  device gpu { kv cap 1000; }
  engine on gpu {
    reqs[2] cap 4;
    tokens cap 64;
    scheduler { advance running; admit waiting; }
    worker { forward (1); }
  }
  pool kv on gpu { }
  pool reqs on Q { }
  entry prefill(n) { hold reqs (cost(reqs, 1)), kv (cost(kv, n)) { run Q prefill (cost(Q, n)) growing kv; } }
}
server (n) { call Q.prefill(n); }",
        "workload { arrive poisson(1); init { set n = 10; } send (n); session { turn; end; } }",
        "`reqs[2]` in queue `Q`'s engine",
    );
}

/// The pool option `admit via` said again what `on` says: it is refused,
/// and the error says how to write each case.
#[test]
fn admit_via_is_refused_and_says_what_to_write() {
    let old = QUEUES.replace("  reqs[2] cap 4;\n", "").replace(
        "pool reqs on llm { }",
        "pool reqs[2] { cap 4; admit via llm; }",
    );
    let e = error_with(&old, QUEUES_WORKLOAD);
    for part in [
        "who admits a pool is said in its `on`",
        "`NAME cap c;` in `engine E { … }`",
        "`pool NAME on E { … }`",
        "`pool NAME on E.DEVICE`",
    ] {
        assert!(e.contains(part), "missing {part:?}:\n{e}");
    }
}

/// Inside a `queue`, `device gpu` is the member's and `engine on gpu` is
/// the queue's stage: `examples/pd-disaggregation/llmd_nixl_pull.sq` writes
/// both pods as engines, each member running vLLM's procedure on its own
/// device's `kv`. The decoder's holds wait in `kv`, which its engine admits
/// (`on D.gpu`); the prefiller's wait in `reqs`, its `kv` admitted as soon
/// as it fits.
#[test]
fn a_queue_holds_its_engine() {
    let base = root().join("examples/pd-disaggregation");
    let p = compiled_example(&base.join("llmd_nixl_pull.sq"), &common::horizon(100.0));
    for (q, kv_admitted) in [("P", false), ("D", true)] {
        // `queue P[NP]`, `queue D[ND]`: one engine per member
        let n = p.stages.iter().filter(|s| s.name == q).count() as u32;
        assert!(n > 1, "{q} has {n} members");
        for i in 0..n {
            let e = stage(&p, q, Some(i));
            let (reqs, kv) = (
                pool(&p, &format!("{q}.reqs"), Some(i)),
                pool(&p, &format!("{q}.kv"), Some(i)),
            );
            let st = step_at(&p, e);
            assert_eq!(Schedule::of(st), procedure(), "{q}[{i}]");
            assert_eq!(st.memory, Some(kv), "{q}[{i}]");
            assert_eq!(p.pools[reqs].admit_via, Some(e), "{q}[{i}]");
            assert_eq!(p.pools[kv].admit_via, kv_admitted.then_some(e), "{q}[{i}]");
        }
    }
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
server (prompt) {{ E.prefill (prompt); }}
"
        )
    };
    let device = "device gpu { t1 (x) = 1; kv cap 10; }";
    let engine = "engine on gpu { tokens cap 4; scheduler { advance running; admit waiting while (running.preempted == 0); } worker { forward (t1(batch.tokens)); } }";
    let w = "workload { arrive batch(1); init { set prompt = 2; } send (prompt); }";
    let ov = common::horizon(10.0);
    compiled_with(
        &pod(&format!("{device} {engine} pool kv on gpu {{ }}")),
        w,
        &ov,
    );
    compiled_with(
        &pod(&format!("{device} {engine} pool kv on E.gpu {{ }}")),
        w,
        &ov,
    );
    refused_with(
        &pod(&format!(
            "{} {engine} pool kv on gpu {{ }}",
            device.replace("gpu", "gpu[2]")
        )),
        w,
        "a queue's device is the member's",
    );
    refused_with(
        &pod(&format!(
            "{device} {engine} serve fifo; pool kv on gpu {{ }}"
        )),
        w,
        "is one stage",
    );
    refused_with(
        &pod(&format!("{engine} {device} pool kv on gpu {{ }}")),
        w,
        "no device `gpu` in queue `E`",
    );
    refused_with(
        &pod(&format!("{device} {engine} {engine} pool kv on gpu {{ }}")),
        w,
        "has one stage",
    );
    // a queue's pool is on its own device or engine, never one outside it
    let other = format!(
        "queue F[3] {{ device g {{ t1 (x) = 1; }} {} }}\n",
        engine.replace("gpu", "g")
    );
    refused_with(
        &format!(
            "{other}{}",
            pod(&format!(
                "{device} {engine} pool kv on gpu {{ }} pool r on F {{ }}"
            ))
        ),
        w,
        "`F` is not a device or the engine of queue `E`",
    );
    refused_with(
        &format!(
            "device gtop {{ t1 (x) = 1; }}\n{}",
            pod(&format!("{device} {engine} pool kv on gtop {{ }}"))
        ),
        w,
        "`gtop` is not a device or the engine of queue `E`",
    );
    refused_with(
        &format!(
            "{}pool reqs on E {{ }}\n",
            pod(&format!("{device} {engine} pool kv on gpu {{ }}"))
        ),
        w,
        "declare `pool reqs on E` in queue `E`",
    );
    // a queue and a top-level device of one name: refused in either order
    let top = "device E { t1 (x) = 1; z cap 2; } pool z on E { }\n";
    let queue = pod(&format!("{device} {engine} pool kv on gpu {{ }}"));
    let (decls, rest) = queue.split_at(queue.find("server").unwrap());
    let device_first =
        "`E` is a device at 1:20; a queue named `E` would make the one word mean two things";
    let queue_first = "a device named `E` would make the one word mean two things";
    refused_with(&format!("{top}{decls}{rest}"), w, device_first);
    refused_with(&format!("{decls}{top}{rest}"), w, queue_first);
    // a gateway has no stage, and is named all the same
    let gateway = "queue E : gateway { route () { } }\n";
    refused_with(&format!("{top}{gateway}"), w, device_first);
    refused_with(&format!("{gateway}{top}"), w, queue_first);
    // the names a declaration outside a queue may not take
    refused_with(
        &pod(&format!("{device} {engine} pool kv on E {{ }}").replace("gpu", "E")),
        w,
        "a queue's device needs another name",
    );
    refused_with(
        &pod(&format!("{device} {engine} pool kv on gpu {{ }}"))
            .replace("queue E", "queue engine")
            .replace("run E", "run engine")
            .replace("cost(E,", "cost(engine,")
            .replace("E.prefill", "engine.prefill"),
        w,
        "a queue's engine is named after the queue",
    );
    refused_with(
        &pod(&format!("{device} {engine} pool kv on gpu {{ }}").replace("gpu", "schedule")),
        w,
        "`schedule` is a word of the language",
    );
    refused_with(
        &pod(&format!("{device} {engine} pool kv on kv {{ }}").replace("gpu", "kv")),
        w,
        "`kv` is a device of queue `E` at 2:10; a pool of queue `E` named `kv` would make the one word mean two things",
    );
    // an error names a queue's device as the program does
    refused_with(
        &pod(&format!("{device} {engine} pool kv on gpu {{ }}")
            .replace(" kv cap 10;", " kv cap 10; enc cap 5;")),
        w,
        "`enc` of `gpu` of queue `E` is no pool: declare `pool enc on gpu { … }` in queue `E`",
    );
    // #403's checks hold for a queue's engine
    refused_with(
        &pod(&format!("{device} {engine} pool kv on gpu {{ }}")).replace(
            "advance running; admit waiting while (running.preempted == 0);",
            "advance running each at most (0);",
        ),
        w,
        "`each at most (0)`",
    );
    refused_with(
        &pod(&format!("{device} {engine} pool kv on gpu {{ }}"))
            .replace("queue E :", "queue E[2] :")
            .replace("E.prefill", "E[0].prefill")
            .replace(
                "while (running.preempted == 0)",
                "while (waiting.count > 0)",
            ),
        w,
        "its `waiting.count` would read every member's",
    );
}

/// An argument a definition does not read is resolved where the use stands
/// (#431), and is not in the IR.
#[test]
fn an_argument_its_definition_drops_is_resolved_where_the_use_stands() {
    let one = |s: String| format!("def one(x) {{ 1 }}\n{s}");
    let ok = "advance running; admit waiting while (running.preempted == 0);";
    let ov = common::horizon(10.0);
    let ir = |src: &str| serde_json::to_string(&compiled(src, &ov)).unwrap();
    // a value of the engine, read in `worker`
    let execute = |e: &str| {
        one(engine(ok)).replace(
            "step_time(batch.tokens)",
            &format!("step_time(batch.tokens) + {e}"),
        )
    };
    // never evaluated, it is not checked for its moment: `prompt` alone
    // would be read for the stage, with no session
    assert_eq!(ir(&execute("one(prompt)")), ir(&execute("1")));
    // a schedule's `let`, read in `each at most`
    let capped = |c: &str| {
        one(engine(&format!(
            "let L = 4; advance running each at most ({c}); admit waiting while \
             (running.preempted == 0) each at most ({c});"
        )))
    };
    assert_eq!(ir(&capped("one(L)")), ir(&capped("1")));
    // the boundary: it is parsed where it stands, and the parser reads
    // `batch.…` as a name the batch has not formed yet in `each at most`
    refused(
        &capped("L + one(batch.tokens)"),
        "`each at most` is read before the batch is formed",
    );
}

/// `each at most` is an engine's clause, so its words are the language's:
/// they were missing from `KEYWORDS` while the engine form read them, and
/// `def each(x) { x }` linked.
#[test]
fn a_def_may_not_be_named_each_or_most() {
    let (src, ov) = example(
        &root().join("docs/tutorial/programs/01-queue.sq"),
        &common::horizon(10.0),
    );
    for w in ["each", "most"] {
        let prog = src.replacen(
            "let S = 1.0;",
            &format!("def {w}(x) {{ x }}\nlet S = 1.0;"),
            1,
        );
        assert_ne!(prog, src);
        refused_example(&prog, &ov, &format!("`{w}` is a word of the language"));
    }
}

/// A power has one spelling, `x ^ y`: a `pow(x, y)` is told so, in a
/// constant and in a statement alike, and a name that is no function is
/// unknown, in a constant too (it read "not a constant function").
#[test]
fn a_power_is_written_with_a_caret() {
    let (src, ov) = example(
        &root().join("docs/tutorial/programs/01-queue.sq"),
        &common::horizon(10.0),
    );
    for (from, to) in [
        ("let S = 1.0;", "let S = pow(1, 1);"),
        ("set t0 = now;", "set t0 = pow(now, 1);"),
    ] {
        let prog = src.replacen(from, to, 1);
        assert_ne!(prog, src);
        refused_example(
            &prog,
            &ov,
            "unknown function `pow`: `pow(x, y)` is now `x ^ y`",
        );
    }
    let prog = src.replacen("let S = 1.0;", "let S = foo(1);", 1);
    refused_example(&prog, &ov, "unknown function `foo`");
    let prog = src.replacen("let S = 1.0;", "let S = 1.0 ^ 1;", 1);
    compile_paired(&prog, &ov).unwrap();
    // the name is free: only a call is told
    let prog = src.replacen("let S = 1.0;", "let pow = 1.0;\nlet S = pow;", 1);
    compile_paired(&prog, &ov).unwrap();
}

/// `price(s, s_hit, ds)` was one paper's price of a miss as a builtin; a
/// program writes it as a `def` over the stage's estimates (#67,
/// `docs/design/miss-price.md`). A call of the builtin is told what to
/// write. The name is free, so a program that defines `price` keeps its
/// calls (the companion programs' migration), and the IR reads the three
/// estimates where it read `Price`, whatever the definition is named. The
/// `def` the reference gives a reader to copy is held to the same formula.
#[test]
fn a_miss_price_is_a_definition_over_the_estimates() {
    let (src, ov) = example(
        &root().join("examples/multi-turn/vllm.sq"),
        &common::horizon(10.0),
    );
    // the definition above `fn main()`, as a program writes one that reads
    // none of main's names (docs/writing-programs.md); the key in the pool
    let priced = |def: &str, call: &str| {
        let main = format!("{def}fn main() {{");
        let key = format!("evict by (waiting, {call} / size);");
        replaced(&src, &[("fn main() {", &main), ("evict lru;", &key)])
    };
    refused_example(
        &priced("", "price(vllm, c0 + a, a * size)"),
        &ov,
        "unknown function `price`: `price(s, s_hit, ds)` is gone",
    );
    const PHI: &str = "ds + est_lambda(s) * ((s_hit + ds) * (s_hit + ds) - s_hit * s_hit) \
                       / (2 * (1 - est_rho(s))) + est_lambda(s) * est_wait(s) * ds / (1 - est_rho(s))";
    let ir = |name: &str| {
        let prog = priced(
            &format!("def {name}(s, s_hit, ds) {{ {PHI} }}\n\n"),
            &format!("{name}(vllm, c0 + a, a * size)"),
        );
        let p = compile_paired(&prog, &ov).unwrap_or_else(|e| panic!("{e}\n{prog}"));
        serde_json::to_string(&p).unwrap()
    };
    // Φ reads λ̂ in its two fractions, ρ̂ in their two denominators, Ŵ once
    let reads_phi = |ir: &str| {
        for (f, n) in [("EstLambda", 2), ("EstRho", 2), ("EstWait", 1)] {
            assert_eq!(ir.matches(&format!("\"{f}\"")).count(), n, "{f}");
        }
    };
    let named = ir("price");
    reads_phi(&named);
    assert_eq!(named, ir("miss_price"));
    // the block docs/api/functions.md gives a reader to copy, its `def`
    // above `fn main()` and its pool in place of the example's: it links,
    // and with its squares (`^`, which is `powf`) written as products it is
    // the definition above
    let doc = std::fs::read_to_string(root().join("docs/api/functions.md")).unwrap();
    let block = doc
        .split_once("{#priced-order}")
        .and_then(|(_, s)| s.split_once("```serq\n"))
        .and_then(|(_, s)| s.split_once("```"))
        .map(|(b, _)| b.trim_end())
        .unwrap();
    let (def, pool) = block.rsplit_once('\n').unwrap();
    let lru = "pool kv on gpu { block bs; evict lru; preempt lifo; }";
    let with = |def: &str, pool: &str| {
        let main = format!("{def}\n\nfn main() {{");
        replaced(&src, &[("fn main() {", &main), (lru, pool)])
    };
    let documented = |def: &str| {
        let prog = with(def, pool);
        let p = compile_paired(&prog, &ov).unwrap_or_else(|e| panic!("{e}\n{prog}"));
        serde_json::to_string(&p).unwrap()
    };
    reads_phi(&documented(def));
    let squares = [(
        "(s_hit + ds) ^ 2 - s_hit ^ 2",
        "(s_hit + ds) * (s_hit + ds) - s_hit * s_hit",
    )];
    assert_eq!(documented(&replaced(def, &squares)), ir("miss_price"));
    // run as docs/design/miss-price.md ran it. Every entry is priced at one
    // stage with the same estimates, and its price per token,
    // a + λ̂ (2 s_hit a + a² size) / 2(1 - ρ̂) + λ̂ Ŵ a / (1 - ρ̂), grows with
    // `size` once λ̂ > 0: the documented order is the order by size, the
    // same report whatever the estimates' values, and not the tie of a
    // constant key
    let mut run = common::horizon(600.0);
    run.set("blocks", "800").unwrap();
    let report = |prog: &str| {
        let r = common::run(prog, &ov.workload, &run).unwrap_or_else(|e| panic!("{e}\n{prog}"));
        r.json()
    };
    let by = |k: &str| {
        let key = "miss_price(vllm, c0 + a, a * size) / size";
        report(&with("", &replaced(pool, &[(key, k)])))
    };
    let by_price = report(&with(def, pool));
    assert_eq!(by_price, by("size"));
    assert_ne!(by_price, by("a"));
}

/// A device's resource is a definition too, replaced token by token: its
/// parameter may not be named like a distribution, as a `def`'s may not.
#[test]
fn a_resource_parameter_may_not_be_named_like_a_distribution() {
    let (src, ov) = example(
        &root().join("docs/tutorial/programs/05-engine.sq"),
        &common::horizon(10.0),
    );
    let prog = src.replacen(
        "compute (t) = t * alpha;",
        "compute (uniform) = uniform * alpha;",
        1,
    );
    assert_ne!(prog, src);
    refused_example(&prog, &ov, "`uniform` is a word of the language");
}

/// An arithmetic function takes numbers alone; one that reads a pool or a
/// stage is not arithmetic. (A function of numbers alone may still read
/// state, so the converse is not a test.)
#[test]
fn an_arithmetic_function_takes_numbers_alone() {
    use serq::ir::{ArgKind, Fun};
    for name in Fun::names() {
        let f = Fun::from_name(name).unwrap();
        if f.is_arithmetic() {
            assert!(f.signature().iter().all(|a| *a == ArgKind::Expr), "{name}");
        }
    }
}

/// A name passed where a function takes a number is read as written
/// (#469): `min(prompt[7], 1000)` linked as `min(prompt, 1000)`. A value
/// has no index, and a pool or stage, whole or a member, is no number;
/// the same holds in a constant, through a definition's parameter, and
/// for an argument the definition does not read. The hold's body is empty,
/// and `E` is a family of stations: an engine family is a queue family.
#[test]
fn a_name_passed_as_a_number_is_read_as_written() {
    let src = |size: &str| {
        format!(
            "def g(x) {{ min(x, 1) }}
def h(x) {{ 1 }}
device gpu[2] {{ kv cap 100; }} pool kv on gpu {{ }}
stage E[2] : fifo;
server (prompt, j) {{ hold kv[j] (cost(kv, {size})) {{ run E[j] (cost(E, 1)); }} }}"
        )
    };
    let w = "workload { arrive batch(1); init { set prompt = 32; set j = 0; } send (prompt, j); }";
    let ov = common::horizon(10.0);
    compiled_with(&src("min(prompt, 1000)"), w, &ov);
    // a value may not share a stage's name, so a bare `E` is the stage (#470)
    let shared = error_with(
        &src("min(E, 1)"),
        &w.replace("set j = 0;", "set j = 0; set E = 3;"),
    );
    assert!(
        shared.contains("a stage named `E` would make the one word mean two things"),
        "{shared}"
    );
    let pool = "as a number, but a pool is none: read";
    let stage = "as a number, but a stage is none: read";
    let attribute = "`prompt[…]`: an attribute has no index: write `prompt`";
    for (size, message) in [
        ("min(prompt[7], 1000)", attribute.to_string()),
        (
            "min(kv[j], 1)",
            format!("`min` reads `kv[…]` {pool} `queued(kv[…])` or `holders(kv[…])`"),
        ),
        (
            "min(E[j], 1)",
            format!("`min` reads `E[…]` {stage} `budget_left(E[…])` or `work(E[…])`"),
        ),
        (
            "min(kv, 1)",
            format!("`min` reads `kv` {pool} `queued(kv)` or `holders(kv)`"),
        ),
        (
            "min(E, 1)",
            format!("`min` reads `E` {stage} `budget_left(E)` or `work(E)`"),
        ),
        (
            "g(kv[j])",
            format!("`min` reads `kv[…]` {pool} `queued(kv[…])`"),
        ),
        ("h(prompt[zz])", attribute.to_string()),
        (
            "min(no_such_name[0], 1)",
            "unknown name `no_such_name`".to_string(),
        ),
    ] {
        refused_with(&src(size), w, &message);
    }
    // a constant's function call: `min(N[1], 4)` was `min(N, 4)`
    refused_with(
        &format!("let N = 2; let A = min(N[1], 4);\n{}", src("A")),
        w,
        "`N[…]`: a number has no index: write `N`",
    );
}

/// `cost`'s last argument takes a number as `min`'s do, and a constant reads
/// a name only where its function takes one (#467): `cost(kv, prompt[7])`
/// was "cost's last argument is an expression", `cost(kv, kv)` an unknown
/// name, and `let A = queued(kv)` said `queued` read `kv` as a number.
#[test]
fn a_name_is_a_number_only_where_its_function_takes_one() {
    let src = |pre: &str, size: &str| {
        format!(
            "{pre}
device gpu {{ kv cap 100; }} pool kv on gpu {{ }}
engine E on gpu {{ tokens cap inf; scheduler {{ advance running; admit waiting while (running.preempted == 0); }} worker {{ forward (1); }} }}
server (prompt) {{ hold kv (cost(kv, {size})) {{ E (1, 1); }} }}"
        )
    };
    let w = "workload { arrive batch(1); init { set prompt = 32; } send (prompt); }";
    refused_with(
        &src("", "prompt[7]"),
        w,
        "`prompt[…]`: an attribute has no index: write `prompt`",
    );
    refused_with(
        &src("", "kv"),
        w,
        "`cost` reads `kv` as a number, but a pool is none",
    );
    refused_with(
        &src("let A = queued(kv);", "A"),
        w,
        "`queued` with 1 argument(s) is not a constant function",
    );
}

/// A device holds one pool, its engine's KV (#645 A3): a second is refused
/// at its `on`, with a code, and the help's way out (the pool on no device,
/// above the engine) links.
#[test]
fn a_second_pool_on_a_device_is_refused_with_its_way_out() {
    let path = root().join("examples/pd-disaggregation/vllm_nixl_push.sq");
    let src = std::fs::read_to_string(&path).unwrap();
    let workload = std::fs::read_to_string(serq::paired_workload(&path).unwrap()).unwrap();
    let kv = "    pool kv on D.gpu { block bs; evict lru; preempt lifo; }\n";
    let device = "    device gpu { kv cap blocksD * bs; }";
    assert!(src.contains(kv) && src.contains(device));
    let two = src
        .replace(kv, &format!("{kv}    pool kv2 on D.gpu {{ block bs; }}\n"))
        .replace(
            device,
            "    device gpu { kv cap blocksD * bs; kv2 cap 160; }",
        );
    let e = common::compile(&two, &workload, &common::horizon(10.0)).unwrap_err();
    assert!(
        e.contains("error[E-device-two-pools]: `D.kv` and `D.kv2` are both on `D.gpu`"),
        "{e}"
    );
    // which is the KV is the program's: the help names both, and the
    // order they are written in chooses neither
    let help = "help: the device an engine runs on holds one pool, its KV: keep";
    assert!(e.contains(help) && e.contains("`D.kv` or `D.kv2`"), "{e}");
    let ahead = src
        .replace(kv, &format!("    pool kv2 on D.gpu {{ block bs; }}\n{kv}"))
        .replace(
            device,
            "    device gpu { kv cap blocksD * bs; kv2 cap 160; }",
        );
    let e = common::compile(&ahead, &workload, &common::horizon(10.0)).unwrap_err();
    assert!(e.contains(help) && e.contains("`D.kv2` or `D.kv`"), "{e}");
    let open = "  queue D : decode {\n";
    assert!(src.contains(open));
    let out = src.replace(open, &format!("{open}    pool kv2 {{ cap 160; }}\n"));
    common::compile(&out, &workload, &common::horizon(10.0)).unwrap_or_else(|e| panic!("{e}"));
}
