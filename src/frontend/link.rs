//! Linking: resolve names to slots and compile expressions.
//!
//! Every `Var` becomes a session-attribute slot, a constant, or a
//! context variable (`size`, `n`, `tokens`, ...). Pool and stage references
//! become base indices plus an optional index expression. Statement blocks
//! are stored in an arena so that a session's continuation is a stack of
//! `(block, pc)` frames.

use std::collections::HashMap;
use std::fmt;

use crate::frontend::ast::*;
use crate::frontend::diagnostic::Source;
use crate::ir::{ArgKind, MAX_SESSIONS};

#[derive(Debug, Clone)]
pub struct LinkError {
    pub message: String,
    pub span: Option<Span>,
}

impl LinkError {
    fn new(message: String) -> Self {
        Self {
            message,
            span: None,
        }
    }

    fn at(mut self, span: Option<Span>) -> Self {
        if self.span.is_none() {
            self.span = span;
        }
        self
    }

    pub fn render(&self, source: &str) -> String {
        self.render_in(source, &[])
    }

    /// Render against the program's text or the library the error is in.
    pub fn render_in(&self, source: &str, libs: &[Source]) -> String {
        match self.span {
            Some(span) => span.render_in(source, libs, &format!("link error: {}", self.message)),
            None => self.to_string(),
        }
    }
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(span) = self.span {
            write!(f, "{}:{}: ", span.line, span.col)?;
        }
        write!(f, "link error: {}", self.message)
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

/// Overrides of a program's `let` constants and expression `def`s: the
/// CLI's `--set` and `--def`, pyserq's `sets=` and `defs=`.
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    pub lets: Vec<(String, Expr)>,
    /// The body of the expression definition `name`,
    /// in place of the program's (a definition expands where it is used,
    /// so this is the program as if written with that body).
    pub defs: Vec<(String, String)>,
    pub horizon: Option<f64>,
    pub warmup: Option<f64>,
    pub seed: Option<u64>,
    pub arrivals: Option<usize>,
    /// Replaces the program's trace file (resolved against the current
    /// directory, not the program's).
    pub trace: Option<String>,
}

impl Overrides {
    /// The constant `name` is the expression `expr`.
    pub fn set(&mut self, name: &str, expr: &str) -> Result<(), String> {
        check_set_name(name)?;
        let e = crate::frontend::parser::parse_expr(expr)
            .map_err(|e| format!("invalid expression in set `{name} = {expr}`: {e}"))?;
        self.lets.push((name.to_string(), e));
        Ok(())
    }

    /// The expression definition `name` has the body
    /// `expr`, which may draw, read attributes and use the definitions
    /// before it, as the program's own body could.
    pub fn define(&mut self, name: &str, expr: &str) -> Result<(), String> {
        check_set_name(name)?;
        crate::frontend::parser::parse_expr(expr).map_err(|e| {
            format!("invalid expression in the `def` override `{name} = {expr}`: {e}")
        })?;
        self.defs.retain(|(n, _)| n != name);
        self.defs.push((name.to_string(), expr.to_string()));
        Ok(())
    }

    /// The constant `name` is the number `x`, exactly (no text round trip).
    /// An infinity is `inf`, as `--set name=inf` writes it; NaN is refused
    /// when the program is linked, as any constant that is NaN.
    pub fn set_num(&mut self, name: &str, x: f64) -> Result<(), String> {
        check_set_name(name)?;
        self.lets.push((name.to_string(), Expr::Num(x)));
        Ok(())
    }
}

fn check_set_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.chars().enumerate().all(|(i, c)| {
            c == '_'
                || if i == 0 {
                    c.is_alphabetic()
                } else {
                    c.is_alphanumeric()
                }
        });
    if ok {
        Ok(())
    } else {
        Err(format!("invalid set name `{name}`; expected an identifier"))
    }
}

struct Linker<'a> {
    consts: HashMap<String, f64>,
    attrs: Vec<String>,
    attr_index: HashMap<String, usize>,
    observes: Vec<String>,
    pools: HashMap<String, (usize, usize)>,
    stages: HashMap<String, (usize, usize)>,
    blocks: Vec<Vec<CStmt>>,
    /// Per block, per statement: where the statement is in the text, as
    /// far as a reference or an expression in it says (#279).
    spans: Vec<Vec<Option<Span>>>,
    prog: &'a Program,
    /// Terms the program's aggregates have written out so far, nested ones
    /// included, against `MAX_OVER`.
    over_terms: std::cell::Cell<usize>,
}

/// The context variables by their source names (`docs/api/context.md`). A
/// name resolves to an attribute first, then a `let`, then one of these, so
/// neither an attribute nor a `let` may take one of these names (`link`
/// rejects it, as it does a `let` and an attribute of one name): the
/// expression that meant the context variable would read the attribute
/// instead (#231).
pub const CONTEXT_VARS: [(&str, CtxVar); 20] = [
    ("now", CtxVar::Now),
    ("waited", CtxVar::Waited),
    ("size", CtxVar::Size),
    ("age", CtxVar::Age),
    ("last", CtxVar::Last),
    ("waiting", CtxVar::Queued),
    ("present", CtxVar::N),
    ("tokens", CtxVar::Ntok),
    ("decoders", CtxVar::Ndec),
    ("prefilled", CtxVar::Npre),
    ("residents", CtxVar::Nres),
    ("kv_decode", CtxVar::Kvb),
    ("kv_prefill", CtxVar::Kvp),
    ("attention", CtxVar::Attn),
    ("decoding", CtxVar::Decoding),
    ("admission", CtxVar::Admission),
    ("remaining", CtxVar::Remaining),
    ("demand", CtxVar::Demand),
    ("served", CtxVar::Served),
    ("arrived", CtxVar::Arrived),
];

/// The most terms the aggregates (`max j in n (e)`) of one program write
/// out, nested ones included.
pub(crate) const MAX_OVER: usize = 4096;

/// Calls the linker folds to a constant from a declaration.
pub const FOLDED: [&str; 1] = ["blocksize"];

/// The functions a call may name, as a constant the parser checks names
/// against and `scripts/metrics.py` counts: the IR's `Fun::names`, which
/// `tests/docs_lexer.rs` holds it to.
pub const FUNCTIONS: [&str; 22] = [
    "min",
    "max",
    "abs",
    "floor",
    "ceil",
    "sqrt",
    "exp",
    "ln",
    "pow",
    "queue",
    "busy",
    "work",
    "used",
    "free",
    "cachedin",
    "holders",
    "queued",
    "price",
    "budget_left",
    "est_lambda",
    "est_rho",
    "est_wait",
];

/// The aggregates of a run's observations, `total(o)` …, which a claim
/// `at end` reads: a call whose argument is an `observe` name.
pub const AGGREGATES: [(&str, crate::ir::Agg); 5] = [
    ("total", crate::ir::Agg::Total),
    ("count", crate::ir::Agg::Count),
    ("largest", crate::ir::Agg::Largest),
    ("smallest", crate::ir::Agg::Smallest),
    ("prefix_total", crate::ir::Agg::PrefixTotal),
];

/// Context variables renamed for what they mean (#139), for a program that
/// still says the old name: it is refused with the new one.
const RENAMED: [(&str, &str); 9] = [
    ("queued", "waiting"),
    ("n", "present"),
    ("ntok", "tokens"),
    ("ndec", "decoders"),
    ("npre", "prefilled"),
    ("nres", "residents"),
    ("kvb", "kv_decode"),
    ("kvp", "kv_prefill"),
    ("attn", "attention"),
];

pub const BUILTIN_ATTRS: [&str; 9] = [
    "cached", "serial", "turn_no", "new", "out", "think", "more", "forced", "computed",
];

pub fn link(prog: &Program, ov: &Overrides) -> LResult<Linked> {
    link_located(prog, ov).map(|(p, _)| p)
}

/// Where each linked statement is in the text: per block, per statement.
pub type Spans = Vec<Vec<Option<Span>>>;

/// `link`, and where each statement of the IR came from, so that an error
/// `Program::validate` finds in a statement can point at the text.
pub fn link_located(prog: &Program, ov: &Overrides) -> LResult<(Linked, Spans)> {
    for (name, _) in &ov.lets {
        if !prog.lets.iter().any(|(declared, _)| declared == name) {
            let names: Vec<_> = prog.lets.iter().map(|(n, _)| n.as_str()).collect();
            return Err(LinkError::new(format!(
                "unknown `let` override `{name}`\nhelp: an override replaces a declared `let`; available constants: {}",
                if names.is_empty() {
                    "(none)".into()
                } else {
                    names.join(", ")
                }
            )));
        }
    }
    let mut lk = Linker {
        consts: HashMap::new(),
        attrs: vec![],
        attr_index: HashMap::new(),
        observes: vec![],
        pools: HashMap::new(),
        stages: HashMap::new(),
        blocks: vec![],
        spans: vec![],
        prog,
        over_terms: std::cell::Cell::new(0),
    };
    for a in BUILTIN_ATTRS {
        lk.attr(a);
    }
    // Names of pools, stages and attributes, before the constants: an
    // aggregate in a `let` checks its index against all of them.
    let mut base = 0;
    for p in &prog.pools {
        if lk.pools.insert(p.name.clone(), (base, p.count)).is_some() {
            return Err(lk.duplicate("pool", &p.name, p.span));
        }
        base += p.count;
    }
    let mut base = 0;
    for s in &prog.stages {
        if lk.stages.insert(s.name.clone(), (base, s.count)).is_some() {
            return Err(lk.duplicate("stage", &s.name, s.span));
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
    // A name the language supplies (a context variable, `inf`) is its own,
    // as it is for a body binding (`parser.rs`) and an aggregate's index
    // (`unroll`). A name resolves to an attribute first, then a `let`, then
    // a context variable, so a `set present = …` anywhere in the program
    // would make a stage's `ps(min(present, 16))` read the attribute, not
    // the jobs present, with no warning (#231: a PS service time off by 8×;
    // before it, the PD lecture's attribute `n`).
    let supplied = CONTEXT_VARS
        .iter()
        .map(|(n, v)| (*n, v.moments()))
        .chain(std::iter::once(("inf", &[][..])));
    for (name, moments) in supplied {
        let taken = if lk.attr_index.contains_key(name) {
            Some("session attribute")
        } else if prog.lets.iter().any(|(n, _)| n == name) {
            Some("`let` constant")
        } else {
            None
        };
        if let Some(kind) = taken {
            let read_in = match moments {
                [] => "every expression".to_string(),
                ms => ms
                    .iter()
                    .map(|m| m.to_string())
                    .collect::<Vec<_>>()
                    .join(", or "),
            };
            return Err(LinkError::new(format!(
                "`{name}` is a name the language supplies, read in {read_in}; a {kind} \
                 named `{name}` would be read there instead\n\
                 help: give the {kind} a name of its own (docs/api/context.md lists the \
                 context variables)"
            )));
        }
    }
    // Constants, in order; an override replaces the value of a `let`.
    for (name, e) in &prog.lets {
        let overridden = ov.lets.iter().any(|(n, _)| n == name);
        let e = ov
            .lets
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, e)| e)
            .unwrap_or(e);
        let what = if overridden {
            "the value".to_string()
        } else {
            format!("`let {name}`")
        };
        let v = lk.const_eval(e, &what).map_err(|mut error| {
            if overridden {
                // These spans refer to the override's expression, not the program.
                error.message = format!("the `let` override `{name}`: {}", error.message);
                error.span = None;
            }
            error
        })?;
        lk.consts.insert(name.clone(), v);
    }
    // An attribute would shadow a constant of the same name everywhere
    // (a stage's cost has no session, so the constant would read as NaN).
    for (name, _) in &prog.lets {
        if lk.attr_index.contains_key(name) {
            return Err(LinkError::new(format!(
                "`{name}` is both a `let` constant and a session attribute"
            )));
        }
    }
    // Pools.
    let mut pools = vec![];
    for p in &prog.pools {
        let cap = lk.const_eval(&p.cap, &format!("pool `{}`: cap", p.name))?;
        let block = p
            .block
            .as_ref()
            .map(|b| lk.const_eval(b, &format!("pool `{}`: block", p.name)))
            .transpose()?;
        if let Some(b) = block
            && b <= 0.0
        {
            return Err(LinkError::new(format!(
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
            QueueOrder::By(keys) => Some(keys.iter().map(|k| lk.expr(k)).collect::<LResult<_>>()?),
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
        let admit_via = p.admit_via.as_ref().map(|n| lk.stage_span(n)).transpose()?;
        for i in 0..p.count {
            // `pool q[N] { admit via S; }` with `stage S[N]`: q[i] is served
            // by S[i]; with one stage, every q[i] by it
            let admit_via = admit_via
                .map(|(b, c)| member(b, c, i, p.count, &p.name, "admit via"))
                .transpose()?;
            pools.push(CPool {
                admit_via,
                name: p.name.clone(),
                index: p.array.then_some(i as u32),
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
                let c = lk.const_eval(c, &format!("stage `{}`: fifo server count", s.name))?;
                if c < 1.0 || c.fract() != 0.0 {
                    return Err(LinkError::new(format!(
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
                only: sp.only.as_ref().map(|e| lk.expr(e)).transpose()?,
                memory: sp.memory.as_ref().map(|m| lk.pool_base(m)).transpose()?,
            }),
        };
        let memory = match &s.kind {
            StageKind::Step(sp) => sp.memory.as_ref().map(|m| lk.pool_span(m)).transpose()?,
            _ => None,
        };
        for i in 0..s.count {
            let mut kind = kind.clone();
            // `stage E[N] : step { memory kv; }` with `pool kv[N]`: E[i]'s
            // memory is kv[i]; with one pool, every E[i]'s is it
            if let (CStageKind::Step(st), Some((b, c))) = (&mut kind, memory) {
                st.memory = Some(member(b, c, i, s.count, &s.name, "memory")?);
            }
            stages.push(CStage {
                name: s.name.clone(),
                index: s.array.then_some(i as u32),
                kind,
            });
        }
    }
    // Workload.
    let (arrival, trace, trace_ordered, init, turn) = match wl {
        None => (CArrival::None, None, false, vec![], vec![]),
        Some(w) => {
            let a = match &w.arrive {
                Arrival::Poisson(e) => CArrival::Poisson(lk.const_eval(e, "the poisson rate")?),
                // a constant gap is folded, and `Program::validate` refuses
                // one that is not a positive time; a drawn one is the run's
                // to check (#269)
                Arrival::Renewal(e) => match lk.eval_const(e) {
                    Ok(gap) => CArrival::Renewal(CExpr::Num(gap)),
                    Err(_) => CArrival::Renewal(lk.expr(e)?),
                },
                Arrival::Closed(e) => {
                    CArrival::Closed(lk.const_count(e, "the closed population", 1, MAX_SESSIONS)?)
                }
                Arrival::Batch(e) => {
                    CArrival::Batch(lk.const_count(e, "the batch size", 1, MAX_SESSIONS)?)
                }
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
    let init = lk.block(&init)?;
    let turn = lk.block(&turn)?;
    let session = lk.block(&prog.session)?;
    let horizon = match (&ov.horizon, &prog.run.horizon) {
        (Some(h), _) => *h,
        (None, Some(e)) => lk.const_eval(e, "the horizon")?,
        (None, None) => return Err(LinkError::new("no horizon (run { horizon T; })".into())),
    };
    let warmup = match (&ov.warmup, &prog.run.warmup) {
        (Some(w), _) => *w,
        (None, Some(e)) => lk.const_eval(e, "the warmup")?,
        (None, None) => 0.0,
    };
    let seed = match (&ov.seed, &prog.run.seed) {
        (Some(s), _) => *s,
        (None, Some(e)) => lk.const_count(e, "the seed", 0, 1 << 53)? as u64,
        (None, None) => 1,
    };
    let arrivals = match (ov.arrivals, &prog.run.arrivals) {
        (Some(n), _) => Some(n),
        (None, Some(e)) => Some(lk.const_count(e, "arrivals", 1, 1 << 53)?),
        (None, None) => None,
    };
    if warmup >= horizon {
        return Err(LinkError::new("warmup must be below the horizon".into()));
    }
    // `hidden` names session attributes the scheduler's expressions may not
    // read; `Program::validate` enforces it by moment and rejects what the
    // scheduler itself sets. A name nothing sets is a mistake, not an attribute
    let mut hidden = vec![];
    if let Some(w) = prog.workload.as_ref() {
        for n in &w.hidden {
            match lk.attr_index.get(n) {
                Some(&i) => hidden.push(i),
                None => {
                    return Err(LinkError::new(format!(
                        "hidden `{n}`: no `set` or `choose` makes it a session attribute"
                    )));
                }
            }
        }
    }
    let mut gauges = vec![];
    for (name, e) in &prog.gauges {
        let expr = lk.expr(e).map_err(|mut err| {
            err.message = format!("gauge `{name}`: {}", err.message);
            err
        })?;
        gauges.push(crate::ir::Gauge {
            name: name.clone(),
            expr,
        });
    }
    let mut claims = vec![];
    for c in &prog.claims {
        let at = |mut err: LinkError| {
            err.message = format!("claim `{}`: {}", c.name, err.message);
            err.at(c.span)
        };
        let given = c
            .given
            .as_ref()
            .map(|e| lk.expr(e))
            .transpose()
            .map_err(at)?;
        let kind = match &c.over {
            ClaimOver::Every(r) => {
                crate::ir::ClaimKind::EveryIteration(lk.claim_stage(r).map_err(at)?)
            }
            ClaimOver::Some(r) => {
                crate::ir::ClaimKind::SomeIteration(lk.claim_stage(r).map_err(at)?)
            }
            ClaimOver::End => crate::ir::ClaimKind::AtEnd,
        };
        let expr = lk.expr(&c.expr).map_err(at)?;
        claims.push(crate::ir::Claim {
            name: c.name.clone(),
            given,
            kind,
            expr,
        });
    }
    let slot = |lk: &Linker, n: &str| lk.attr_index[n];
    let linked = Linked {
        gauges,
        claims,
        version: IR_VERSION,
        hidden,
        share: prog.share,
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
        arrivals,
    };
    crate::frontend::lint::lint(&linked).map_err(LinkError::new)?;
    Ok((linked, lk.spans))
}

/// Where a statement is: its first reference's, else its first located
/// expression's; a statement with neither (`loop`, `turn`, `end`, `set x =
/// 1`) has none, and an error about it is shown without a line.
fn stmt_span(s: &Stmt) -> Option<Span> {
    fn expr_span(e: &Expr) -> Option<Span> {
        match e {
            Expr::Located(span, _) => Some(*span),
            Expr::Num(_) | Expr::Var(_) => None,
            Expr::Unary(_, a) => expr_span(a),
            Expr::Binary(_, a, b) | Expr::Over(_, _, a, b) => expr_span(a).or_else(|| expr_span(b)),
            Expr::Cond(c, a, b) => expr_span(c)
                .or_else(|| expr_span(a))
                .or_else(|| expr_span(b)),
            Expr::Sample(_, xs) => xs.iter().find_map(expr_span),
            Expr::Call(_, args) => args.iter().find_map(|a| match a {
                Arg::Expr(x) => expr_span(x),
                Arg::Ref(r) => r.span,
            }),
        }
    }
    match s {
        Stmt::Hold { pools, .. } => pools
            .first()
            .and_then(|(r, e, _)| r.span.or_else(|| expr_span(e))),
        Stmt::Grow(r, e) | Stmt::Load(r, e) => r.span.or_else(|| expr_span(e)),
        Stmt::Drop(r) | Stmt::Release(r) => r.span,
        Stmt::Run { stage, work, .. } => stage.span.or_else(|| expr_span(work)),
        Stmt::Set(_, e) | Stmt::Observe(_, e) | Stmt::Branch(e, _, _) => expr_span(e),
        Stmt::Choose { count, .. } => expr_span(count),
        _ => None,
    }
}

/// Member `i` of an `n`-family's counterpart in a family of `count` from
/// `base`: element for element when the counts match, the one member when
/// there is one, and an error otherwise.
fn member(base: usize, count: usize, i: usize, n: usize, who: &str, what: &str) -> LResult<usize> {
    if count == n {
        Ok(base + i)
    } else if count == 1 {
        Ok(base)
    } else {
        Err(LinkError::new(format!(
            "`{who}`: `{what}` names a family of {count}, and `{who}` is a family of {n}: \
             one for one, or one for all"
        )))
    }
}

fn collect_attrs(stmts: &[Stmt], lk: &mut Linker) {
    let mut names = vec![];
    crate::frontend::parser::assigned_in(stmts, &mut names);
    for n in &names {
        lk.attr(n);
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

    fn declaration(&self, kind: &str, name: &str) -> Option<Span> {
        match kind {
            "pool" => self
                .prog
                .pools
                .iter()
                .find(|d| d.name == name)
                .and_then(|d| d.span),
            "stage" => self
                .prog
                .stages
                .iter()
                .find(|d| d.name == name)
                .and_then(|d| d.span),
            _ => self
                .prog
                .definitions
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, s)| *s),
        }
    }

    /// `line:col`, with the library's path when the span is in one.
    fn place(&self, span: Span) -> String {
        match span.file.checked_sub(1).and_then(|i| self.prog.libs.get(i)) {
            Some(lib) => format!("{}:{}:{}", lib.path, span.line, span.col),
            None => format!("{}:{}", span.line, span.col),
        }
    }

    fn duplicate(&self, kind: &str, name: &str, span: Option<Span>) -> LinkError {
        let mut message = format!("duplicate {kind} `{name}`");
        if let Some(first) = self.declaration(kind, name) {
            message.push_str(&format!("\nnote: first declared at {}", self.place(first)));
        }
        message.push_str("\nhelp: rename or remove the duplicate declaration");
        LinkError::new(message).at(span)
    }

    fn unknown(&self, kind: &str, name: &str) -> LinkError {
        let names: Vec<&str> = match kind {
            "pool" => self.pools.keys().map(String::as_str).collect(),
            "stage" => self.stages.keys().map(String::as_str).collect(),
            "constant" => self.consts.keys().map(String::as_str).collect(),
            _ => self
                .consts
                .keys()
                .chain(self.attr_index.keys())
                .map(String::as_str)
                .collect(),
        };
        let mut message = if kind == "constant" {
            format!("`{name}` is not a constant")
        } else {
            format!("unknown {kind} `{name}`")
        };
        if let Some(candidate) = crate::frontend::diagnostic::suggestion(name, names.into_iter()) {
            message.push_str(&format!("\nhelp: did you mean {kind} `{candidate}`?"));
            if let Some(span) = self.declaration(kind, &candidate) {
                message.push_str(&format!(
                    "\nnote: `{candidate}` declared at {}",
                    self.place(span)
                ));
            }
        } else if kind == "name" {
            message.push_str(
                "\nhelp: declare a `let` constant or assign a session attribute before using it",
            );
        } else {
            message.push_str(&format!(
                "\nhelp: declare this {kind} or use the name of an existing {kind}"
            ));
        }
        LinkError::new(message)
    }

    /// A pool family: (base, count), with the reference's diagnostic location.
    fn pool_span(&self, r: &Ref) -> LResult<(usize, usize)> {
        self.pools
            .get(&r.name)
            .copied()
            .ok_or_else(|| self.unknown("pool", &r.name).at(r.span))
    }

    /// A stage family: (base, count), with the reference's diagnostic location.
    fn stage_span(&self, r: &Ref) -> LResult<(usize, usize)> {
        self.stages
            .get(&r.name)
            .copied()
            .ok_or_else(|| self.unknown("stage", &r.name).at(r.span))
    }

    fn pool_base(&self, r: &Ref) -> LResult<usize> {
        self.pool_span(r).map(|(base, _)| base)
    }

    fn stage_base(&self, r: &Ref) -> LResult<usize> {
        self.stage_span(r).map(|(base, _)| base)
    }

    fn cref(&self, r: &Ref, table: &HashMap<String, (usize, usize)>, what: &str) -> LResult<CRef> {
        let &(base, count) = table
            .get(&r.name)
            .ok_or_else(|| self.unknown(what, &r.name).at(r.span))?;
        let index = match &r.index {
            None => {
                if count != 1 {
                    return Err(LinkError::new(format!(
                        "{what} `{}` is an array; index it",
                        r.name
                    )));
                }
                None
            }
            Some(e) => {
                let before = self.over_terms.get();
                let i = self.expr(e)?;
                let after = self.over_terms.get();
                // a constant index (`kv[2]`, `kv[-1]`, `kv[N + 1]`, or one an
                // aggregate wrote out) is checked here, where the program
                // still has its names; one that reads state links as before
                // the fold writes the index's aggregates out again: against
                // the budget they met the first time, and without spending it
                self.over_terms.set(before);
                let folded = match i {
                    // already a number (`blocksize(p)` folds here, not in `eval_const`)
                    CExpr::Num(k) => Some(k),
                    _ => (!e.draws()).then(|| self.eval_const(e).ok()).flatten(),
                };
                self.over_terms.set(after);
                if let Some(k) = folded
                    && !(k >= 0.0 && k.fract() == 0.0 && k < count as f64)
                {
                    return Err(LinkError::new(format!(
                        "{what} `{}[{k}]` is out of range: `{}` has {count} member(s)",
                        r.name, r.name
                    ))
                    .at(r.span));
                }
                // a constant index is a number in the IR: a gauge accepts no other
                Some(Box::new(folded.map_or(i, CExpr::Num)))
            }
        };
        Ok(CRef { base, count, index })
    }

    /// The stage a claim over iterations names: one stage, or a member of
    /// an array by a constant index (`E[0]`).
    fn claim_stage(&self, r: &Ref) -> LResult<usize> {
        let cr = self.stage_ref(r)?;
        match cr.index.as_deref() {
            None => Ok(cr.base),
            Some(CExpr::Num(k)) => Ok(cr.base + *k as usize),
            Some(_) => Err(LinkError::new(format!(
                "the stage `{}[…]` of a claim is a member named by a constant index",
                r.name
            ))
            .at(r.span)),
        }
    }

    fn pool_ref(&self, r: &Ref) -> LResult<CRef> {
        self.cref(r, &self.pools, "pool")
    }

    fn stage_ref(&self, r: &Ref) -> LResult<CRef> {
        self.cref(r, &self.stages, "stage")
    }

    /// `blocksize(p)`: the `block` of pool `p`, a constant the linker folds,
    /// so a definition takes the pool and not its block size beside it.
    fn blocksize(&self, args: &[Arg]) -> LResult<f64> {
        let r = match args {
            [Arg::Ref(r)] => r,
            [Arg::Expr(_)] => {
                return Err(LinkError::new(
                    "`blocksize` takes a pool, not an expression".into(),
                ));
            }
            _ => {
                return Err(LinkError::new(format!(
                    "`blocksize` takes one pool, got {} argument(s)",
                    args.len()
                )));
            }
        };
        // the reference as any pool function's: an array is indexed, and the
        // index links (every member has the declaration's block)
        self.pool_ref(r)?;
        if r.index.as_deref().is_some_and(Expr::draws) {
            return Err(LinkError::new(
                "`blocksize`'s index draws, and the folded number would drop the draw".into(),
            )
            .at(r.span));
        }
        let d = self
            .prog
            .pools
            .iter()
            .find(|d| d.name == r.name)
            .ok_or_else(|| self.unknown("pool", &r.name).at(r.span))?;
        match &d.block {
            Some(b) => self.const_eval(b, &format!("`blocksize({})`", r.name)),
            None => Err(LinkError::new(format!(
                "`blocksize({})`: pool `{}` has no `block`",
                r.name, r.name
            ))
            .at(r.span)),
        }
    }

    /// The constant `what` (`pool \`kv\`: cap`, `the horizon`, ...): a number or
    /// an infinity. No run means anything with a NaN constant, and the IR
    /// has no spelling for one.
    fn const_eval(&self, e: &Expr, what: &str) -> LResult<f64> {
        let v = self.eval_const(e)?;
        if v.is_nan() {
            // where the expression has a place (a call, a name), the error has it
            let span = match e {
                Expr::Located(span, _) => Some(*span),
                _ => None,
            };
            return Err(LinkError::new(format!(
                "{what} is NaN\nhelp: a constant is a number or `inf`; `0/0` and `inf - inf` are not"
            ))
            .at(span));
        }
        Ok(v)
    }

    /// A constant that counts (sessions, arrivals): a whole number from `min`
    /// to `max`. A cast would have made -1 a 0, 2.5 a 2 and 1e30 a run that
    /// never starts (#289).
    fn const_count(&self, e: &Expr, what: &str, min: usize, max: usize) -> LResult<usize> {
        let v = self.const_eval(e, what)?;
        if !(v.fract() == 0.0 && v >= min as f64 && v <= max as f64) {
            return Err(LinkError::new(format!(
                "{what} is {v}: a count is a whole number from {min} to {max}"
            )));
        }
        Ok(v as usize)
    }

    /// Evaluate a constant expression (no attributes, no samples).
    fn eval_const(&self, e: &Expr) -> LResult<f64> {
        Ok(match e {
            Expr::Located(span, inner) => self.eval_const(inner).map_err(|e| e.at(Some(*span)))?,
            Expr::Num(x) => *x,
            Expr::Var(n) => match n.as_str() {
                "inf" => f64::INFINITY,
                _ => *self
                    .consts
                    .get(n)
                    .ok_or_else(|| self.unknown("constant", n))?,
            },
            Expr::Unary(UnOp::Neg, a) => -self.eval_const(a)?,
            Expr::Unary(UnOp::Not, a) => {
                if self.eval_const(a)? != 0.0 {
                    0.0
                } else {
                    1.0
                }
            }
            Expr::Binary(op, a, b) => binop(*op, self.eval_const(a)?, self.eval_const(b)?),
            Expr::Cond(c, a, b) => {
                if self.eval_const(c)? != 0.0 {
                    self.eval_const(a)?
                } else {
                    self.eval_const(b)?
                }
            }
            Expr::Call(f, _) if f == "blocksize" => {
                return Err(LinkError::new(
                    "`blocksize` is not a constant: in a constant position (a `let`, `cap`, \
                     `block`, an array size, `horizon`, a rate) name the value with a `let` \
                     both use"
                        .into(),
                ));
            }
            Expr::Call(f, args) => {
                let xs: Vec<f64> = args
                    .iter()
                    .map(|a| match a {
                        Arg::Expr(e) => self.eval_const(e),
                        Arg::Ref(r) => self
                            .eval_const(&Expr::Var(r.name.clone()))
                            .map_err(|e| e.at(r.span)),
                    })
                    .collect::<LResult<_>>()?;
                match const_call(f, &xs) {
                    Some(x) => x,
                    None => {
                        return Err(LinkError::new(format!(
                            "`{f}` with {} argument(s) is not a constant function",
                            xs.len()
                        )));
                    }
                }
            }
            Expr::Sample(..) => return Err(LinkError::new("a constant cannot sample".into())),
            Expr::Over(agg, j, n, body) => self.eval_const(&self.unroll(*agg, j, n, body)?)?,
        })
    }

    /// `max j in n (e)` written out: `e` with `j` = 0, 1, …, n-1, folded by
    /// binary `max`, `min` or `+`. `n` is a constant, and `j` a name of its
    /// own: a constant, an attribute, a pool, a stage or a context variable
    /// of the same name would leave the body saying two things.
    fn unroll(&self, agg: Agg, j: &str, n: &Expr, body: &Expr) -> LResult<Expr> {
        let what = format!("`{} {j} in`", agg.name());
        let clash = if self.prog.lets.iter().any(|(n, _)| n == j) {
            Some("a `let` constant")
        } else if self.attr_index.contains_key(j) {
            Some("a session attribute")
        } else if self.pools.contains_key(j) {
            Some("a pool")
        } else if self.stages.contains_key(j) {
            Some("a stage")
        } else if CONTEXT_VARS.iter().any(|(name, _)| *name == j) || j == "inf" {
            Some("a name the language supplies")
        } else {
            None
        };
        if let Some(c) = clash {
            return Err(LinkError::new(format!(
                "{what}: `{j}` is also {c}\nhelp: give the index a name of its own"
            )));
        }
        let count = self.const_eval(n, &format!("{what}'s count"))?;
        if !(count >= 1.0 && count.fract() == 0.0 && count.is_finite()) {
            return Err(LinkError::new(format!(
                "{what}'s count must be a positive integer, found {count}"
            )));
        }
        // the terms are written out, nested aggregates' included: the
        // program's total is bounded, so no count makes the linker run away
        let total = self.over_terms.get() as f64 + count;
        if total > MAX_OVER as f64 {
            return Err(LinkError::new(format!(
                "{what}: the program's aggregates write out {total} terms, at most {MAX_OVER}"
            )));
        }
        self.over_terms.set(total as usize);
        let term = |k: usize| {
            let mut e = body.clone();
            bind_index(&mut e, j, k as f64);
            e
        };
        let mut acc = term(0);
        for k in 1..count as usize {
            let t = term(k);
            acc = match agg {
                Agg::Sum => Expr::Binary(BinOp::Add, Box::new(acc), Box::new(t)),
                Agg::Max | Agg::Min => {
                    Expr::Call(agg.name().into(), vec![Arg::Expr(acc), Arg::Expr(t)])
                }
            };
        }
        Ok(acc)
    }

    fn expr(&self, e: &Expr) -> LResult<CExpr> {
        Ok(match e {
            Expr::Located(span, inner) => self.expr(inner).map_err(|e| e.at(Some(*span)))?,
            Expr::Num(x) => CExpr::Num(*x),
            Expr::Var(n) => {
                if let Some(&i) = self.attr_index.get(n) {
                    CExpr::Attr(i)
                } else if let Some(&v) = self.consts.get(n) {
                    CExpr::Num(v)
                } else {
                    if let Some(&(_, v)) = CONTEXT_VARS.iter().find(|(name, _)| name == n) {
                        CExpr::Ctx(v)
                    } else if n == "inf" {
                        CExpr::Num(f64::INFINITY)
                    } else if let Some((_, new)) = RENAMED.iter().find(|(old, _)| old == n) {
                        return Err(LinkError::new(format!(
                            "the context variable `{n}` is now `{new}`"
                        )));
                    } else {
                        return Err(self.unknown("name", n));
                    }
                }
            }
            Expr::Sample(d, args) => {
                let Some(kind) = DistKind::from_name(d) else {
                    return Err(LinkError::new(format!("unknown distribution `{d}`")));
                };
                let want = kind.arity();
                if args.len() != want {
                    return Err(LinkError::new(format!("`~{d}` takes {want} argument(s)")));
                }
                CExpr::Sample(
                    kind,
                    args.iter().map(|a| self.expr(a)).collect::<LResult<_>>()?,
                )
            }
            Expr::Call(f, args) if f == "blocksize" => CExpr::Num(self.blocksize(args)?),
            Expr::Call(f, args) if AGGREGATES.iter().any(|(n, _)| n == f) => {
                let agg = AGGREGATES.iter().find(|(n, _)| n == f).expect("guarded").1;
                let [Arg::Ref(r)] = args.as_slice() else {
                    return Err(LinkError::new(format!(
                        "`{f}` takes the name of an `observe`, as `{f}(latency)`"
                    )));
                };
                let k = (r.index.is_none())
                    .then(|| self.observes.iter().position(|o| *o == r.name))
                    .flatten()
                    .ok_or_else(|| {
                        LinkError::new(format!(
                            "`{f}({})`: `{}` is not an `observe` of the program",
                            r.name, r.name
                        ))
                        .at(r.span)
                    })?;
                CExpr::Agg(agg, k)
            }
            Expr::Call(f, args) => {
                let Some(fun) = Fun::from_name(f) else {
                    return Err(LinkError::new(format!("unknown function `{f}`")));
                };
                let sig = fun.signature();
                if args.len() != sig.len() {
                    return Err(LinkError::new(format!(
                        "`{f}` takes {} argument(s), got {}",
                        sig.len(),
                        args.len()
                    )));
                }
                let mut cargs = vec![];
                for (a, kind) in args.iter().zip(sig) {
                    cargs.push(match (kind, a) {
                        (ArgKind::Expr, Arg::Expr(e)) => CArg::Expr(self.expr(e)?),
                        (ArgKind::Expr, Arg::Ref(r)) => CArg::Expr(
                            self.expr(&Expr::Var(r.name.clone()))
                                .map_err(|e| e.at(r.span))?,
                        ),
                        (ArgKind::Pool, Arg::Ref(r)) => CArg::Pool(self.pool_ref(r)?),
                        (ArgKind::Stage, Arg::Ref(r)) => CArg::Stage(self.stage_ref(r)?),
                        (k, _) => {
                            let what = if *k == ArgKind::Pool { "pool" } else { "stage" };
                            return Err(LinkError::new(format!(
                                "`{f}` expects a {what} name here"
                            )));
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
            Expr::Over(agg, j, n, body) => self.expr(&self.unroll(*agg, j, n, body)?)?,
        })
    }

    fn observe_slot(&mut self, name: &str) -> usize {
        if let Some(i) = self.observes.iter().position(|o| o == name) {
            return i;
        }
        self.observes.push(name.to_string());
        self.observes.len() - 1
    }

    /// Compile a block into the arena and return its id.
    fn block(&mut self, stmts: &[Stmt]) -> LResult<BlockId> {
        let id = self.blocks.len();
        self.blocks.push(vec![]);
        self.spans.push(stmts.iter().map(stmt_span).collect());
        let mut out = vec![];
        for s in stmts {
            let cs = match s {
                Stmt::Set(n, e) => CStmt::Set(self.attr_index[n], self.expr(e)?),
                Stmt::Observe(n, e) => {
                    let e = self.expr(e)?;
                    CStmt::Observe(self.observe_slot(n), e)
                }
                Stmt::Turn => CStmt::Turn,
                Stmt::End => CStmt::End,
                Stmt::Request => {
                    return Err(LinkError::new(
                        "`request` survived parsing: the parser splices the server in its place"
                            .into(),
                    ));
                }
                Stmt::Call { .. } | Stmt::Mark(_) => {
                    return Err(LinkError::new(
                        "a queue's entry call survived parsing: the parser expands it in place"
                            .into(),
                    ));
                }
                Stmt::Hold {
                    pools,
                    reuse,
                    body,
                    cache,
                    lease,
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
                    let body = self.block(body)?;
                    let lease = match lease {
                        None => None,
                        Some((r, t)) => Some((self.pool_ref(r)?, self.expr(t)?)),
                    };
                    CStmt::Hold {
                        pools,
                        reuse,
                        body,
                        cache,
                        lease,
                    }
                }
                Stmt::Grow(r, e) => CStmt::Grow(self.pool_ref(r)?, self.expr(e)?),
                Stmt::Drop(r) => CStmt::Drop(self.pool_ref(r)?),
                Stmt::Release(r) => CStmt::Release(self.pool_ref(r)?),
                Stmt::Load(r, e) => CStmt::Load(self.pool_ref(r)?, self.expr(e)?),
                Stmt::Run {
                    stage,
                    mode,
                    work,
                    growing,
                    also,
                } => {
                    let stage = self.stage_ref(stage)?;
                    let also = also
                        .iter()
                        .map(|r| self.stage_ref(r))
                        .collect::<LResult<Vec<_>>>()?;
                    CStmt::Run {
                        stage,
                        mode: *mode,
                        work: self.expr(work)?,
                        growing: growing.as_ref().map(|g| self.pool_ref(g)).transpose()?,
                        also,
                    }
                }
                Stmt::Branch(p, a, b) => {
                    let p = self.expr(p)?;
                    let a = self.block(a)?;
                    let b = self.block(b)?;
                    CStmt::Branch(p, a, b)
                }
                Stmt::Loop(b) => CStmt::Loop(self.block(b)?),
                Stmt::Choose { var, count, key } => CStmt::Choose {
                    var: self.attr_index[var],
                    count: self.expr(count)?,
                    key: key.iter().map(|k| self.expr(k)).collect::<LResult<_>>()?,
                },
            };
            out.push(cs);
        }
        self.blocks[id] = out;
        Ok(id)
    }
}

/// A constant function of constants: what a `let`, a `cap` or an array size
/// may call. The parser folds a family's size with it, the linker every
/// constant.
pub fn const_call(f: &str, xs: &[f64]) -> Option<f64> {
    Some(match (f, xs) {
        ("min", [a, b]) => a.min(*b),
        ("max", [a, b]) => a.max(*b),
        ("abs", [a]) => a.abs(),
        ("floor", [a]) => a.floor(),
        ("ceil", [a]) => a.ceil(),
        ("sqrt", [a]) => a.sqrt(),
        ("exp", [a]) => a.exp(),
        ("ln", [a]) => a.ln(),
        ("pow", [a, b]) => a.powf(*b),
        _ => return None,
    })
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

/// Replace the index `j` of an `Expr::Over` by the number `k`, in the
/// body and in its references' indices; an inner `Over` of the same name
/// keeps its own.
pub(crate) fn bind_index(e: &mut Expr, j: &str, k: f64) {
    e.substitute(&[(j.to_string(), Expr::Num(k))]);
}
