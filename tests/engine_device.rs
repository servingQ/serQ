//! Engines on devices (`docs/design/engine-device.md`): `device`, `engine …
//! on`, `pool … on`, `schedule` and `execute` are parse-time sugar for the
//! kernel's step stage (`CStageKind::Step`), which no program writes any
//! more, so each test reads the step an engine lowers to from the compiled
//! `Program`; and every rule the design refuses does not link.

mod common;

use std::path::Path;

use serq::ir::{
    BinOp, CArg, CExpr, CIter, CRef, CServe, CStageKind, CStep, CtxVar, Fun, Program, Register,
    UnOp,
};
use serq::{Overrides, compile_source_at};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn compiled(src: &str, base: Option<&Path>, ov: &Overrides) -> Program {
    compile_source_at(&common::main_source(src), base, ov).unwrap_or_else(|e| panic!("{e}\n{src}"))
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
/// examples' constants: the `execute` of the three engines below.
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
/// budget, `execute (c0 + …)` the cost, and the per-run cap the kernel's
/// `chunk`, applied only while more than one request is in the engine
/// (vLLM's `long_prefill_token_threshold`): `inf`, no cap, is its 0, and a cap of 512 is 512.
#[test]
fn vllm_as_an_engine_is_the_procedure() {
    let text = std::fs::read_to_string(root().join("examples/multi-turn/vllm.sq")).unwrap();
    let base = root().join("examples/multi-turn");
    let ov = common::horizon(100.0);
    let chunk = |cap: &str| {
        let src = replaced(
            &text,
            &[(
                r#"args.number("chunk_cap", inf)"#,
                &format!(r#"args.number("chunk_cap", {cap})"#),
            )],
        );
        let p = compiled(&src, Some(&base), &ov);
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
    let tgi = compiled(
        &std::fs::read_to_string(base.join("tgi.sq")).unwrap(),
        Some(&base),
        &ov,
    );
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
    // `advance running; branch (just == 0 || running.count == 0) { admit
    // waiting; } set just = waiting.admitted > 0;`
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

    let sglang = compiled(
        &std::fs::read_to_string(base.join("sglang.sq")).unwrap(),
        Some(&base),
        &ov,
    );
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
            // branch (running.count == 0 && backlog == 0) { set ratio = r0; }
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
            // branch (batch.tokens == 0) { advance running; branch … }
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

/// The step `engine(schedule)` lowers to.
fn lowered(schedule: &str) -> Schedule {
    let p = compiled(&engine(schedule), None, &common::horizon(20.0));
    Schedule::of(step(&p, "vllm"))
}

/// Each form of the kernel's step is a schedule: the procedure is no body,
/// an order is the serve keys, a common `only` is the body `serve only`
/// is, `exclusive prefill` is its rule and a per-run cap the `chunk`; any
/// other schedule is its body, as written.
#[test]
fn the_stage_forms_are_schedules() {
    let p = compiled(
        &engine("advance running; admit waiting while (running.preempted == 0);"),
        None,
        &common::horizon(20.0),
    );
    // the engine's clauses: `tokens cap` is the budget, `execute` the cost,
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
    for (schedule, kernel) in [
        (
            "advance running; admit waiting while (running.preempted == 0);",
            procedure(),
        ),
        (
            "advance running; admit waiting while (!running.preempted);",
            procedure(),
        ),
        (
            "advance running decode first; admit waiting while (running.preempted == 0);",
            Schedule {
                keys: Some(vec![CExpr::Cond(
                    Box::new(decoding()),
                    Box::new(num(0.0)),
                    Box::new(num(1.0)),
                )]),
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
            "advance running; branch (running.count == 0) { admit waiting; }",
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
/// engine reads `running.decoding` in `tokens cap` and `schedule` and
/// `batch.decoding` in `execute` (#416).
#[test]
fn an_engine_reads_its_lists_in_every_clause() {
    let ov = common::horizon(20.0);
    let ok = "advance running; admit waiting while (running.preempted == 0);";
    let reqs = pool(&compiled(&engine(ok), None, &ov), "reqs", None);
    // each value as the kernel's context variable it reads: in `tokens
    // cap 8 + v`, the budget `8 + v`; in `execute (step_time(…) + v)`, the
    // cost `1 + v`
    let tokens_cap = |v: &str| {
        let p = compiled(
            &replaced(
                &engine(ok),
                &[("tokens cap 8;", &format!("tokens cap 8 + {v};"))],
            ),
            None,
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
                    "execute (step_time(batch.tokens));",
                    &format!("execute (step_time(batch.tokens) + {v});"),
                )],
            ),
            None,
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
        ("running.count", ctx(CtxVar::Nres)),
        ("waiting.count", queued(reqs)),
    ] {
        assert_eq!(execute(v), kernel, "execute {v}");
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
    let tokens_below_8 = || bin(BinOp::Lt, ctx(CtxVar::Ntok), num(8.0));
    assert_eq!(
        lowered("advance running; branch (batch.tokens < 8) { admit waiting; }").body,
        Some(vec![
            serve(),
            CIter::Branch(tokens_below_8(), vec![admit()], vec![])
        ])
    );
    // and after an `only`, read for each resident, the statements read it again
    assert_eq!(
        lowered(
            "advance running only (running.count > 0); branch (batch.tokens < 8) { admit \
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
        assert_eq!(
            Schedule::of(step(&compiled(&src, None, &ov), "vllm")),
            kernel
        );
    }
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
        "takes its capacity from it",
    );
    refused(
        &engine(ok).replace(
            "pool reqs on vllm { }",
            "pool reqs on vllm { admit via vllm; }",
        ),
        "who admits a pool on a device or an engine is said in its `on`",
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
    // a pool on a name nothing declares is pointed at that name, the owner
    // (line 9, `pool kv on gpuu`, column 12), not the pool's
    let e = error(&engine(ok).replace("pool kv on gpu", "pool kv on gpuu"));
    assert!(e.starts_with("9:12: no device or engine `gpuu`"), "{e}");
    assert!(e.contains("did you mean `gpu`?"), "{e}");
    // a context variable's old name is answered with the engine's name
    let e = error(&engine(ok).replace("execute (step_time(batch.tokens))", "execute (ntok)"));
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
            "execute (step_time(batch.tokens))",
            &format!("execute (step_time(batch.tokens) + {c} * batch.attention)"),
        )
    };
    let ov = common::horizon(10.0);
    assert_eq!(
        step(
            &compiled(&format!("let attn = 1e-3;\n{}", with("attn")), None, &ov),
            "vllm"
        )
        .cost,
        step(
            &compiled(&format!("let attn = 1e-3;\n{}", with("1e-3")), None, &ov),
            "vllm"
        )
        .cost,
    );
}

/// A family of devices: `pool kv on vllm.gpu` is `kv[N]`, an engine `E[N]`
/// reads `kv[i]` from `E[i]` as its memory and admits its queue; `pool kv
/// on gpu` is the same memory, admitted as soon as it fits.
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
pool kv on vllm.gpu {{ }}
{work}"
    );
    for (src, admitted) in [
        (new.clone(), true),
        (new.replace("on vllm.gpu", "on gpu"), false),
    ] {
        let p = compiled(&src, None, &ov);
        for i in [0, 1] {
            let (e, kv) = (stage(&p, "vllm", Some(i)), pool(&p, "kv", Some(i)));
            assert_eq!(step_at(&p, e).memory, Some(kv), "{src}");
            assert_eq!(p.pools[kv].cap, 100.0);
            assert_eq!(p.pools[kv].admit_via, admitted.then_some(e), "{src}");
        }
    }
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

/// `on ENGINE.DEVICE` names the engine on the device, declared above the
/// pool: that engine admits the pool's queue.
#[test]
fn a_pool_names_the_engine_that_admits_it() {
    let ok = engine("advance running; admit waiting while (running.preempted == 0);");
    let ov = common::horizon(20.0);
    let p = compiled(
        &ok.replace("pool kv on gpu", "pool kv on vllm.gpu"),
        None,
        &ov,
    );
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
        "`on ENGINE.DEVICE` are admitted by the engine",
    );
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
    let p = compiled(
        &std::fs::read_to_string(base.join("llmd_nixl_pull.sq")).unwrap(),
        Some(&base),
        &common::horizon(100.0),
    );
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
workload {{ arrive batch(1); init {{ set prompt = 2; }} }}
server {{ E.prefill (prompt); }}
"
        )
    };
    let device = "device gpu { t1 (x) = 1; kv cap 10; }";
    let engine = "engine on gpu { tokens cap 4; schedule { advance running; admit waiting while (running.preempted == 0); } execute (t1(batch.tokens)); }";
    let ov = common::horizon(10.0);
    compiled(
        &pod(&format!("{device} {engine} pool kv on gpu {{ }}")),
        None,
        &ov,
    );
    compiled(
        &pod(&format!("{device} {engine} pool kv on E.gpu {{ }}")),
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

/// An argument a definition does not read is resolved where the use stands
/// (#431), and is not in the IR.
#[test]
fn an_argument_its_definition_drops_is_resolved_where_the_use_stands() {
    let one = |s: String| format!("def one(x) {{ 1 }}\n{s}");
    let ok = "advance running; admit waiting while (running.preempted == 0);";
    let ov = common::horizon(10.0);
    let ir = |src: &str| serde_json::to_string(&compiled(src, None, &ov)).unwrap();
    // a value of the engine, read in `execute`
    let execute = |e: &str| {
        one(engine(ok)).replace(
            "step_time(batch.tokens)",
            &format!("step_time(batch.tokens) + {e}"),
        )
    };
    // never evaluated, it is not checked for its moment: `prompt` alone
    // would be read for the stage, with no session
    for unread in ["one(waiting.count)", "one(prompt)"] {
        assert_eq!(ir(&execute(unread)), ir(&execute("1")));
    }
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
