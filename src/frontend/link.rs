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

/// Overrides from the command line (`--set name=expr`).
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    pub lets: Vec<(String, Expr)>,
    /// `--def name=expr`: the body of the expression definition `name`,
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
    /// `--set name=expr`: the constant `name` is the expression `expr`.
    pub fn set(&mut self, name: &str, expr: &str) -> Result<(), String> {
        check_set_name(name)?;
        let e = crate::frontend::parser::parse_expr(expr)
            .map_err(|e| format!("invalid expression in set `{name} = {expr}`: {e}"))?;
        self.lets.push((name.to_string(), e));
        Ok(())
    }

    /// `--def name=expr`: the expression definition `name` has the body
    /// `expr`, which may draw, read attributes and use the definitions
    /// before it, as the program's own body could.
    pub fn define(&mut self, name: &str, expr: &str) -> Result<(), String> {
        check_set_name(name)?;
        crate::frontend::parser::parse_expr(expr)
            .map_err(|e| format!("invalid expression in --def `{name} = {expr}`: {e}"))?;
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
    /// The pool references of the holds enclosing the statement being
    /// linked, as written, for `release` and `load`, which act on an
    /// enclosing hold: `hold kv[i]` encloses `release kv[i]`, not `kv[j]`.
    held: Vec<Ref>,
    /// Pool references some hold of the program leases, as written: a
    /// `release` of one may stand outside any hold of it.
    leased: Vec<Ref>,
    prog: &'a Program,
}

/// The context variables by their source names (`docs/api/context.md`). A
/// name resolves to an attribute first, then a `let`, then one of these.
pub const CONTEXT_VARS: [(&str, CtxVar); 17] = [
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
];

/// Calls the linker folds to a constant from a declaration.
pub const FOLDED: [&str; 1] = ["blocksize"];

/// The functions a call may name, as the linker resolves them below.
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
    for (name, _) in &ov.lets {
        if !prog.lets.iter().any(|(declared, _)| declared == name) {
            let names: Vec<_> = prog.lets.iter().map(|(n, _)| n.as_str()).collect();
            return Err(LinkError::new(format!(
                "unknown --set constant `{name}`\nhelp: --set overrides a declared `let`; available constants: {}",
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
        held: vec![],
        leased: vec![],
        prog,
    };
    for a in BUILTIN_ATTRS {
        lk.attr(a);
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
                // These spans refer to the --set expression, not the program.
                error.message = format!("--set {name}: {}", error.message);
                error.span = None;
            }
            error
        })?;
        lk.consts.insert(name.clone(), v);
    }
    // Names of pools and stages.
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
    collect_leases(&prog.session, &mut lk.leased);
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
                Arrival::Renewal(e) => CArrival::Renewal(lk.expr(e)?),
                Arrival::Closed(e) => {
                    CArrival::Closed(lk.const_eval(e, "the closed population")? as usize)
                }
                Arrival::Batch(e) => CArrival::Batch(lk.const_eval(e, "the batch size")? as usize),
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
        (None, Some(e)) => lk.const_eval(e, "the seed")? as u64,
        (None, None) => 1,
    };
    let arrivals = match (ov.arrivals, &prog.run.arrivals) {
        (Some(n), _) => Some(n),
        (None, Some(e)) => Some(lk.const_eval(e, "arrivals")? as usize),
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
    let slot = |lk: &Linker, n: &str| lk.attr_index[n];
    let linked = Linked {
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
    Ok(linked)
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

/// Every pool reference a hold leases, anywhere in the session.
fn collect_leases(stmts: &[Stmt], out: &mut Vec<Ref>) {
    for s in stmts {
        match s {
            Stmt::Hold { body, lease, .. } => {
                if let Some((r, _)) = lease
                    && !out.iter().any(|held| held.same_target(r))
                {
                    out.push(r.clone());
                }
                collect_leases(body, out);
            }
            Stmt::Loop(body) => collect_leases(body, out),
            Stmt::Branch(_, a, b) => {
                collect_leases(a, out);
                collect_leases(b, out);
            }
            _ => {}
        }
    }
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
            Some(e) => Some(Box::new(self.expr(e)?)),
        };
        Ok(CRef { base, count, index })
    }

    /// A pool reference a statement acts on through an enclosing hold
    /// (`release`, `load`): the statement must be inside a hold that names
    /// the pool the same way, index included, or it would look for a hold
    /// the session may not have. A `release` may also take a lease, so it
    /// stands anywhere when some hold leases the pool (`or_leased`).
    fn enclosed_pool(&self, r: &Ref, what: &str, or_leased: bool) -> LResult<CRef> {
        let cr = self.pool_ref(r)?;
        if !self.held.iter().any(|held| held.same_target(r))
            && !(or_leased && self.leased.iter().any(|leased| leased.same_target(r)))
        {
            let shown = match &r.index {
                None => r.name.clone(),
                Some(_) => format!("{}[…]", r.name),
            };
            let hint = if self
                .held
                .iter()
                .chain(&self.leased)
                .any(|h| h.name == r.name)
            {
                ": write the pool as the hold does, index included"
            } else if or_leased {
                ": it takes an enclosing hold's allocation, or a lease of it"
            } else {
                ": it acts on an enclosing hold's allocation"
            };
            return Err(LinkError::new(format!(
                "`{what} {shown}` outside a hold of `{shown}`{hint}"
            )));
        }
        Ok(cr)
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
        if r.index.as_deref().is_some_and(has_draw) {
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
        })
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
                let kind = match d.as_str() {
                    "exp" => DistKind::Exp,
                    "det" => DistKind::Det,
                    "uniform" => DistKind::Uniform,
                    "erlang" => DistKind::Erlang,
                    "h2" => DistKind::H2,
                    "bernoulli" => DistKind::Bernoulli,
                    _ => return Err(LinkError::new(format!("unknown distribution `{d}`"))),
                };
                let want = match kind {
                    DistKind::Exp | DistKind::Det | DistKind::Bernoulli => 1,
                    DistKind::Uniform | DistKind::Erlang | DistKind::H2 => 2,
                };
                if args.len() != want {
                    return Err(LinkError::new(format!("`~{d}` takes {want} argument(s)")));
                }
                CExpr::Sample(
                    kind,
                    args.iter().map(|a| self.expr(a)).collect::<LResult<_>>()?,
                )
            }
            Expr::Call(f, args) if f == "blocksize" => CExpr::Num(self.blocksize(args)?),
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
                    _ => return Err(LinkError::new(format!("unknown function `{f}`"))),
                };
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
                        (&"e", Arg::Expr(e)) => CArg::Expr(self.expr(e)?),
                        (&"e", Arg::Ref(r)) => CArg::Expr(
                            self.expr(&Expr::Var(r.name.clone()))
                                .map_err(|e| e.at(r.span))?,
                        ),
                        (&"p", Arg::Ref(r)) => CArg::Pool(self.pool_ref(r)?),
                        (&"s", Arg::Ref(r)) => CArg::Stage(self.stage_ref(r)?),
                        (k, _) => {
                            let what = if *k == "p" { "pool" } else { "stage" };
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
                    return Err(LinkError::new(
                        "workload blocks may only `set` and `observe`".into(),
                    ));
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
                    let pools_src = pools;
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
                    let depth = self.held.len();
                    for (r, _, _) in pools_src {
                        self.held.push(r.clone());
                    }
                    let body = self.block(body, false)?;
                    self.held.truncate(depth);
                    let lease = match lease {
                        None => None,
                        Some((r, t)) => {
                            if !pools_src.iter().any(|(q, _, _)| q.same_target(r)) {
                                return Err(LinkError::new(format!(
                                    "`lease {}`: the hold does not take that pool (write it as the hold does, index included)",
                                    r.name
                                )));
                            }
                            Some((self.pool_ref(r)?, self.expr(t)?))
                        }
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
                Stmt::Release(r) => {
                    let cr = self.enclosed_pool(r, "release", true)?;
                    CStmt::Release(cr)
                }
                Stmt::Load(r, e) => {
                    let cr = self.enclosed_pool(r, "load", false)?;
                    CStmt::Load(cr, self.expr(e)?)
                }
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
                        return Err(LinkError::new(
                            "`prefill`/`decode` are required on a step stage and not allowed elsewhere"
                                .into(),
                        ));
                    }
                    if growing.is_some() && !is_step {
                        return Err(LinkError::new("`growing` needs a step stage".into()));
                    }
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
                    let a = self.block(a, false)?;
                    let b = self.block(b, false)?;
                    CStmt::Branch(p, a, b)
                }
                Stmt::Loop(b) => CStmt::Loop(self.block(b, false)?),
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

/// Does this expression draw?
fn has_draw(e: &Expr) -> bool {
    match e {
        Expr::Located(_, inner) => has_draw(inner),
        Expr::Sample(..) => true,
        Expr::Num(_) | Expr::Var(_) => false,
        Expr::Call(_, args) => args.iter().any(|a| match a {
            Arg::Expr(x) => has_draw(x),
            Arg::Ref(r) => r.index.as_deref().is_some_and(has_draw),
        }),
        Expr::Unary(_, a) => has_draw(a),
        Expr::Binary(_, a, b) => has_draw(a) || has_draw(b),
        Expr::Cond(c, a, b) => has_draw(c) || has_draw(a) || has_draw(b),
    }
}
