//! Linking: resolve names to slots and compile expressions.
//!
//! Every `Var` becomes a session-attribute slot, a constant, or a
//! context variable (`size`, `n`, `ntok`, ...). Pool and stage references
//! become base indices plus an optional index expression. Statement blocks
//! are stored in an arena so that a session's continuation is a stack of
//! `(block, pc)` frames.

use std::collections::HashMap;
use std::fmt;

use crate::ast::*;

#[derive(Debug, Clone)]
pub struct LinkError(pub String);

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "link error: {}", self.0)
    }
}

impl std::error::Error for LinkError {}

type LResult<T> = Result<T, LinkError>;

pub use crate::ir::{
    BlockId, CArg, CArrival, CEvict, CExpr, CPool, CRef, CServe, CSpill, CStage, CStageKind, CStep,
    CStmt, CtxVar, DistKind, Fun, IR_VERSION, SessionInit,
};

/// The linked program is the IR (`crate::ir::Program`); the old name stays.
pub type Linked = crate::ir::Program;

/// Overrides from the command line (`--set name=expr`).
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    pub lets: Vec<(String, Expr)>,
    pub horizon: Option<f64>,
    pub warmup: Option<f64>,
    pub seed: Option<u64>,
    /// Replaces the program's trace file (resolved against the current
    /// directory, not the program's).
    pub trace: Option<String>,
}

struct Linker<'a> {
    consts: HashMap<String, f64>,
    attrs: Vec<String>,
    attr_index: HashMap<String, usize>,
    observes: Vec<String>,
    pools: HashMap<String, (usize, usize)>,
    stages: HashMap<String, (usize, usize)>,
    blocks: Vec<Vec<CStmt>>,
    prog: &'a Program,
}

pub const BUILTIN_ATTRS: [&str; 9] = [
    "cached", "serial", "turn_no", "new", "out", "think", "more", "forced", "computed",
];

pub fn link(prog: &Program, ov: &Overrides) -> LResult<Linked> {
    let mut lk = Linker {
        consts: HashMap::new(),
        attrs: vec![],
        attr_index: HashMap::new(),
        observes: vec![],
        pools: HashMap::new(),
        stages: HashMap::new(),
        blocks: vec![],
        prog,
    };
    for a in BUILTIN_ATTRS {
        lk.attr(a);
    }
    // Constants, in order; an override replaces the value of a `let`.
    for (name, e) in &prog.lets {
        let e = ov
            .lets
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, e)| e)
            .unwrap_or(e);
        let v = lk.const_eval(e)?;
        lk.consts.insert(name.clone(), v);
    }
    for (name, e) in &ov.lets {
        if !lk.consts.contains_key(name) {
            let v = lk.const_eval(e)?;
            lk.consts.insert(name.clone(), v);
        }
    }
    // Names of pools and stages.
    let mut base = 0;
    for p in &prog.pools {
        if lk.pools.insert(p.name.clone(), (base, p.count)).is_some() {
            return Err(LinkError(format!("duplicate pool `{}`", p.name)));
        }
        base += p.count;
    }
    let mut base = 0;
    for s in &prog.stages {
        if lk.stages.insert(s.name.clone(), (base, s.count)).is_some() {
            return Err(LinkError(format!("duplicate stage `{}`", s.name)));
        }
        base += s.count;
    }
    // Attributes: everything assigned anywhere.
    let wl = prog.workload.as_ref();
    if let Some(w) = wl {
        collect_attrs(&w.init, &mut lk);
        collect_attrs(&w.turn, &mut lk);
    }
    collect_attrs(&prog.session, &mut lk);
    // An attribute would shadow a constant of the same name everywhere
    // (a stage's cost has no session, so the constant would read as NaN).
    for (name, _) in &prog.lets {
        if lk.attr_index.contains_key(name) {
            return Err(LinkError(format!(
                "`{name}` is both a `let` constant and a session attribute"
            )));
        }
    }
    // Pools.
    let mut pools = vec![];
    for p in &prog.pools {
        let cap = lk.const_eval(&p.cap)?;
        let block = p.block.as_ref().map(|b| lk.const_eval(b)).transpose()?;
        if let Some(b) = block
            && b <= 0.0
        {
            return Err(LinkError(format!(
                "pool `{}`: block must be positive",
                p.name
            )));
        }
        let evict = match &p.evict {
            EvictOrder::Lru => CEvict::Lru,
            EvictOrder::By(keys) => {
                CEvict::By(keys.iter().map(|k| lk.expr(k)).collect::<LResult<_>>()?)
            }
        };
        let queue = match &p.queue {
            QueueOrder::Fifo => None,
            QueueOrder::By(e) => Some(lk.expr(e)?),
        };
        let spill = match &p.spill {
            None => None,
            Some(s) => {
                let to = lk.pool_base(&s.to)?;
                let via = lk.stage_base(&s.via)?;
                Some(CSpill {
                    to,
                    via,
                    work: lk.expr(&s.work)?,
                    when: lk.expr(&s.when)?,
                })
            }
        };
        let admit_via = p.admit_via.as_ref().map(|n| lk.stage_base(n)).transpose()?;
        for _ in 0..p.count {
            pools.push(CPool {
                admit_via,
                name: p.name.clone(),
                cap,
                block,
                evict: evict.clone(),
                preempt: p.preempt,
                queue: queue.clone(),
                spill: spill.clone(),
            });
        }
    }
    // Stages.
    let mut stages = vec![];
    for s in &prog.stages {
        let kind = match &s.kind {
            StageKind::Fifo(c) => {
                let c = lk.const_eval(c)?;
                if c < 1.0 || c.fract() != 0.0 {
                    return Err(LinkError(format!(
                        "stage `{}`: fifo servers must be a positive integer",
                        s.name
                    )));
                }
                CStageKind::Fifo(c as usize)
            }
            StageKind::Ps(phi) => CStageKind::Ps(lk.expr(phi)?),
            StageKind::Delay => CStageKind::Delay,
            StageKind::Step(sp) => CStageKind::Step(CStep {
                budget: lk.expr(&sp.budget)?,
                cost: lk.expr(&sp.cost)?,
                chunk: lk.expr(&sp.chunk)?,
                serve: match &sp.serve {
                    // no keys: every resident ties, and ties are admission order
                    Serve::Admission => CServe::By(vec![]),
                    // `decode first` is `by (decoding ? 0 : 1)`: the IR knows one form
                    Serve::DecodeFirst => CServe::By(vec![CExpr::Cond(
                        Box::new(CExpr::Ctx(CtxVar::Decoding)),
                        Box::new(CExpr::Num(0.0)),
                        Box::new(CExpr::Num(1.0)),
                    )]),
                    Serve::By(keys) => {
                        CServe::By(keys.iter().map(|k| lk.expr(k)).collect::<Result<_, _>>()?)
                    }
                    Serve::ExclusivePrefill => CServe::ExclusivePrefill,
                },
                memory: sp.memory.as_ref().map(|m| lk.pool_base(m)).transpose()?,
            }),
        };
        for _ in 0..s.count {
            stages.push(CStage {
                name: s.name.clone(),
                kind: kind.clone(),
            });
        }
    }
    // Workload.
    let (arrival, trace, trace_ordered, init, turn) = match wl {
        None => (CArrival::None, None, false, vec![], vec![]),
        Some(w) => {
            let a = match &w.arrive {
                Arrival::Poisson(e) => CArrival::Poisson(lk.const_eval(e)?),
                Arrival::Closed(e) => CArrival::Closed(lk.const_eval(e)? as usize),
                Arrival::Batch(e) => CArrival::Batch(lk.const_eval(e)? as usize),
                Arrival::None => CArrival::None,
            };
            (
                a,
                w.trace.clone(),
                w.trace_ordered,
                w.init.clone(),
                w.turn.clone(),
            )
        }
    };
    let init = lk.block(&init, true)?;
    let turn = lk.block(&turn, true)?;
    let session = lk.block(&prog.session, false)?;
    let horizon = match (&ov.horizon, &prog.run.horizon) {
        (Some(h), _) => *h,
        (None, Some(e)) => lk.const_eval(e)?,
        (None, None) => return Err(LinkError("no horizon (run { horizon T; })".into())),
    };
    let warmup = match (&ov.warmup, &prog.run.warmup) {
        (Some(w), _) => *w,
        (None, Some(e)) => lk.const_eval(e)?,
        (None, None) => 0.0,
    };
    let seed = match (&ov.seed, &prog.run.seed) {
        (Some(s), _) => *s,
        (None, Some(e)) => lk.const_eval(e)? as u64,
        (None, None) => 1,
    };
    if warmup >= horizon {
        return Err(LinkError("warmup must be below the horizon".into()));
    }
    let slot = |lk: &Linker, n: &str| lk.attr_index[n];
    let linked = Linked {
        version: IR_VERSION,
        slot_cached: slot(&lk, "cached"),
        slot_serial: slot(&lk, "serial"),
        slot_turn: slot(&lk, "turn_no"),
        slot_new: slot(&lk, "new"),
        slot_out: slot(&lk, "out"),
        slot_think: slot(&lk, "think"),
        slot_more: slot(&lk, "more"),
        slot_forced: slot(&lk, "forced"),
        slot_computed: slot(&lk, "computed"),
        attrs: lk.attrs,
        observes: lk.observes,
        pools,
        stages,
        arrival,
        trace,
        trace_ordered,
        init,
        turn,
        session,
        blocks: lk.blocks,
        horizon,
        warmup,
        seed,
    };
    crate::lint::lint(&linked).map_err(LinkError)?;
    Ok(linked)
}

fn collect_attrs(stmts: &[Stmt], lk: &mut Linker) {
    for s in stmts {
        match s {
            Stmt::Set(n, _) | Stmt::Choose { var: n, .. } => {
                lk.attr(n);
            }
            Stmt::Hold { body, .. } | Stmt::Loop(body) => collect_attrs(body, lk),
            Stmt::Branch(_, a, b) => {
                collect_attrs(a, lk);
                collect_attrs(b, lk);
            }
            _ => {}
        }
    }
}

impl Linker<'_> {
    fn attr(&mut self, name: &str) -> usize {
        if let Some(&i) = self.attr_index.get(name) {
            return i;
        }
        let i = self.attrs.len();
        self.attrs.push(name.to_string());
        self.attr_index.insert(name.to_string(), i);
        i
    }

    fn pool_base(&self, name: &str) -> LResult<usize> {
        self.pools
            .get(name)
            .map(|&(b, _)| b)
            .ok_or_else(|| LinkError(format!("unknown pool `{name}`")))
    }

    fn stage_base(&self, name: &str) -> LResult<usize> {
        self.stages
            .get(name)
            .map(|&(b, _)| b)
            .ok_or_else(|| LinkError(format!("unknown stage `{name}`")))
    }

    fn cref(&self, r: &Ref, table: &HashMap<String, (usize, usize)>, what: &str) -> LResult<CRef> {
        let &(base, count) = table
            .get(&r.name)
            .ok_or_else(|| LinkError(format!("unknown {what} `{}`", r.name)))?;
        let index = match &r.index {
            None => {
                if count != 1 {
                    return Err(LinkError(format!(
                        "{what} `{}` is an array; index it",
                        r.name
                    )));
                }
                None
            }
            Some(e) => Some(Box::new(self.expr(e)?)),
        };
        Ok(CRef { base, count, index })
    }

    fn pool_ref(&self, r: &Ref) -> LResult<CRef> {
        self.cref(r, &self.pools, "pool")
    }

    fn stage_ref(&self, r: &Ref) -> LResult<CRef> {
        self.cref(r, &self.stages, "stage")
    }

    /// Evaluate a constant expression (no attributes, no samples).
    fn const_eval(&self, e: &Expr) -> LResult<f64> {
        Ok(match e {
            Expr::Num(x) => *x,
            Expr::Var(n) => match n.as_str() {
                "inf" => f64::INFINITY,
                _ => *self
                    .consts
                    .get(n)
                    .ok_or_else(|| LinkError(format!("`{n}` is not a constant")))?,
            },
            Expr::Unary(UnOp::Neg, a) => -self.const_eval(a)?,
            Expr::Unary(UnOp::Not, a) => {
                if self.const_eval(a)? != 0.0 {
                    0.0
                } else {
                    1.0
                }
            }
            Expr::Binary(op, a, b) => binop(*op, self.const_eval(a)?, self.const_eval(b)?),
            Expr::Cond(c, a, b) => {
                if self.const_eval(c)? != 0.0 {
                    self.const_eval(a)?
                } else {
                    self.const_eval(b)?
                }
            }
            Expr::Call(f, args) => {
                let xs: Vec<f64> = args
                    .iter()
                    .map(|a| match a {
                        Arg::Expr(e) => self.const_eval(e),
                        Arg::Ref(r) => self.const_eval(&Expr::Var(r.name.clone())),
                    })
                    .collect::<LResult<_>>()?;
                match (f.as_str(), xs.as_slice()) {
                    ("min", [a, b]) => a.min(*b),
                    ("max", [a, b]) => a.max(*b),
                    ("abs", [a]) => a.abs(),
                    ("floor", [a]) => a.floor(),
                    ("ceil", [a]) => a.ceil(),
                    ("sqrt", [a]) => a.sqrt(),
                    ("exp", [a]) => a.exp(),
                    ("ln", [a]) => a.ln(),
                    ("pow", [a, b]) => a.powf(*b),
                    _ => {
                        return Err(LinkError(format!(
                            "`{f}` with {} argument(s) is not a constant function",
                            xs.len()
                        )));
                    }
                }
            }
            Expr::Sample(..) => return Err(LinkError("a constant cannot sample".into())),
        })
    }

    fn expr(&self, e: &Expr) -> LResult<CExpr> {
        Ok(match e {
            Expr::Num(x) => CExpr::Num(*x),
            Expr::Var(n) => {
                if let Some(&i) = self.attr_index.get(n) {
                    CExpr::Attr(i)
                } else if let Some(&v) = self.consts.get(n) {
                    CExpr::Num(v)
                } else {
                    match n.as_str() {
                        "now" => CExpr::Ctx(CtxVar::Now),
                        "size" => CExpr::Ctx(CtxVar::Size),
                        "age" => CExpr::Ctx(CtxVar::Age),
                        "last" => CExpr::Ctx(CtxVar::Last),
                        "queued" => CExpr::Ctx(CtxVar::Queued),
                        "n" => CExpr::Ctx(CtxVar::N),
                        "ntok" => CExpr::Ctx(CtxVar::Ntok),
                        "ndec" => CExpr::Ctx(CtxVar::Ndec),
                        "npre" => CExpr::Ctx(CtxVar::Npre),
                        "nres" => CExpr::Ctx(CtxVar::Nres),
                        "kvb" => CExpr::Ctx(CtxVar::Kvb),
                        "kvp" => CExpr::Ctx(CtxVar::Kvp),
                        "attn" => CExpr::Ctx(CtxVar::Attn),
                        "decoding" => CExpr::Ctx(CtxVar::Decoding),
                        "admission" => CExpr::Ctx(CtxVar::Admission),
                        "remaining" => CExpr::Ctx(CtxVar::Remaining),
                        "inf" => CExpr::Num(f64::INFINITY),
                        _ => return Err(LinkError(format!("unknown name `{n}`"))),
                    }
                }
            }
            Expr::Sample(d, args) => {
                let kind = match d.as_str() {
                    "exp" => DistKind::Exp,
                    "det" => DistKind::Det,
                    "uniform" => DistKind::Uniform,
                    "erlang" => DistKind::Erlang,
                    "h2" => DistKind::H2,
                    "bernoulli" => DistKind::Bernoulli,
                    _ => return Err(LinkError(format!("unknown distribution `{d}`"))),
                };
                let want = match kind {
                    DistKind::Exp | DistKind::Det | DistKind::Bernoulli => 1,
                    DistKind::Uniform | DistKind::Erlang | DistKind::H2 => 2,
                };
                if args.len() != want {
                    return Err(LinkError(format!("`~{d}` takes {want} argument(s)")));
                }
                CExpr::Sample(
                    kind,
                    args.iter().map(|a| self.expr(a)).collect::<LResult<_>>()?,
                )
            }
            Expr::Call(f, args) => {
                let (fun, sig): (Fun, &[&str]) = match f.as_str() {
                    "min" => (Fun::Min, &["e", "e"]),
                    "max" => (Fun::Max, &["e", "e"]),
                    "abs" => (Fun::Abs, &["e"]),
                    "floor" => (Fun::Floor, &["e"]),
                    "ceil" => (Fun::Ceil, &["e"]),
                    "sqrt" => (Fun::Sqrt, &["e"]),
                    "exp" => (Fun::Exp, &["e"]),
                    "ln" => (Fun::Ln, &["e"]),
                    "pow" => (Fun::Pow, &["e", "e"]),
                    "queue" => (Fun::Queue, &["s"]),
                    "busy" => (Fun::Busy, &["s"]),
                    "work" => (Fun::Work, &["s"]),
                    "used" => (Fun::Used, &["p"]),
                    "free" => (Fun::Free, &["p"]),
                    "cachedin" => (Fun::CachedIn, &["p"]),
                    "holders" => (Fun::Holders, &["p"]),
                    "queued" => (Fun::Queued, &["p"]),
                    "price" => (Fun::Price, &["s", "e", "e"]),
                    "budget_left" => (Fun::BudgetLeft, &["s"]),
                    "est_lambda" => (Fun::EstLambda, &["s"]),
                    "est_rho" => (Fun::EstRho, &["s"]),
                    "est_wait" => (Fun::EstWait, &["s"]),
                    _ => return Err(LinkError(format!("unknown function `{f}`"))),
                };
                if args.len() != sig.len() {
                    return Err(LinkError(format!(
                        "`{f}` takes {} argument(s), got {}",
                        sig.len(),
                        args.len()
                    )));
                }
                let mut cargs = vec![];
                for (a, kind) in args.iter().zip(sig) {
                    cargs.push(match (kind, a) {
                        (&"e", Arg::Expr(e)) => CArg::Expr(self.expr(e)?),
                        (&"e", Arg::Ref(r)) => CArg::Expr(self.expr(&Expr::Var(r.name.clone()))?),
                        (&"p", Arg::Ref(r)) => CArg::Pool(self.pool_ref(r)?),
                        (&"s", Arg::Ref(r)) => CArg::Stage(self.stage_ref(r)?),
                        (k, _) => {
                            let what = if *k == "p" { "pool" } else { "stage" };
                            return Err(LinkError(format!("`{f}` expects a {what} name here")));
                        }
                    });
                }
                CExpr::Call(fun, cargs)
            }
            Expr::Unary(op, a) => CExpr::Unary(*op, Box::new(self.expr(a)?)),
            Expr::Binary(op, a, b) => {
                CExpr::Binary(*op, Box::new(self.expr(a)?), Box::new(self.expr(b)?))
            }
            Expr::Cond(c, a, b) => CExpr::Cond(
                Box::new(self.expr(c)?),
                Box::new(self.expr(a)?),
                Box::new(self.expr(b)?),
            ),
        })
    }

    fn observe_slot(&mut self, name: &str) -> usize {
        if let Some(i) = self.observes.iter().position(|o| o == name) {
            return i;
        }
        self.observes.push(name.to_string());
        self.observes.len() - 1
    }

    /// Compile a block into the arena and return its id. `workload` blocks
    /// may only assign and observe.
    fn block(&mut self, stmts: &[Stmt], workload: bool) -> LResult<BlockId> {
        let id = self.blocks.len();
        self.blocks.push(vec![]);
        let mut out = vec![];
        for s in stmts {
            let cs = match s {
                Stmt::Set(n, e) => CStmt::Set(self.attr_index[n], self.expr(e)?),
                Stmt::Observe(n, e) => {
                    let e = self.expr(e)?;
                    CStmt::Observe(self.observe_slot(n), e)
                }
                _ if workload => {
                    return Err(LinkError(
                        "workload blocks may only `set` and `observe`".into(),
                    ));
                }
                Stmt::Turn => CStmt::Turn,
                Stmt::End => CStmt::End,
                Stmt::Request => {
                    return Err(LinkError(
                        "`request` survived parsing: the parser splices the server in its place"
                            .into(),
                    ));
                }
                Stmt::Hold {
                    pools,
                    reuse,
                    body,
                    cache,
                } => {
                    let pools = pools
                        .iter()
                        .map(|(r, e, f)| {
                            Ok((
                                self.pool_ref(r)?,
                                self.expr(e)?,
                                f.as_ref().map(|f| self.expr(f)).transpose()?,
                            ))
                        })
                        .collect::<LResult<Vec<_>>>()?;
                    let reuse = reuse.as_ref().map(|c| self.expr(c)).transpose()?;
                    let cache = cache.as_ref().map(|c| self.expr(c)).transpose()?;
                    let body = self.block(body, false)?;
                    CStmt::Hold {
                        pools,
                        reuse,
                        body,
                        cache,
                    }
                }
                Stmt::Grow(r, e) => CStmt::Grow(self.pool_ref(r)?, self.expr(e)?),
                Stmt::Drop(r) => CStmt::Drop(self.pool_ref(r)?),
                Stmt::Run {
                    stage,
                    mode,
                    work,
                    growing,
                } => {
                    let stage = self.stage_ref(stage)?;
                    let is_step = matches!(
                        self.prog.stages.iter().find(|s| {
                            self.stages.get(&s.name).map(|b| b.0) == Some(stage.base)
                        }),
                        Some(StageDecl {
                            kind: StageKind::Step(_),
                            ..
                        })
                    );
                    if is_step != (*mode != RunMode::Plain) {
                        return Err(LinkError(
                            "`prefill`/`decode` are required on a step stage and not allowed elsewhere"
                                .into(),
                        ));
                    }
                    if growing.is_some() && !is_step {
                        return Err(LinkError("`growing` needs a step stage".into()));
                    }
                    CStmt::Run {
                        stage,
                        mode: *mode,
                        work: self.expr(work)?,
                        growing: growing.as_ref().map(|g| self.pool_ref(g)).transpose()?,
                    }
                }
                Stmt::Branch(p, a, b) => {
                    let p = self.expr(p)?;
                    let a = self.block(a, false)?;
                    let b = self.block(b, false)?;
                    CStmt::Branch(p, a, b)
                }
                Stmt::Loop(b) => CStmt::Loop(self.block(b, false)?),
                Stmt::Choose { var, count, key } => CStmt::Choose {
                    var: self.attr_index[var],
                    count: self.expr(count)?,
                    key: self.expr(key)?,
                },
            };
            out.push(cs);
        }
        self.blocks[id] = out;
        Ok(id)
    }
}

pub fn binop(op: BinOp, a: f64, b: f64) -> f64 {
    let t = |c: bool| if c { 1.0 } else { 0.0 };
    match op {
        BinOp::Add => a + b,
        BinOp::Sub => a - b,
        BinOp::Mul => a * b,
        BinOp::Div => a / b,
        BinOp::Pow => a.powf(b),
        BinOp::Lt => t(a < b),
        BinOp::Le => t(a <= b),
        BinOp::Gt => t(a > b),
        BinOp::Ge => t(a >= b),
        BinOp::Eq => t(a == b),
        BinOp::Ne => t(a != b),
        BinOp::And => t(a != 0.0 && b != 0.0),
        BinOp::Or => t(a != 0.0 || b != 0.0),
    }
}
