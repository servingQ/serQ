//! Linking: resolve names to slots and compile expressions.
//!
//! Every `Var` becomes a session-attribute slot, a constant, or a
//! context variable (`size`, `n`, `tokens`, ...). Pool and stage references
//! become base indices plus an optional index expression. Statement blocks
//! are stored in an arena so that a session's continuation is a stack of
//! `(block, pc)` frames.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use crate::frontend::ast::*;
use crate::ir::{ArgKind, CIter, MAX_SESSIONS, Moment};

#[derive(Debug, Clone)]
pub struct LinkError {
    pub message: String,
    pub span: Option<Span>,
    /// The rule's code, for an editor (`diagnostic::Code`).
    pub code: Option<crate::frontend::diagnostic::Code>,
}

impl LinkError {
    pub(crate) fn new(message: String) -> Self {
        Self {
            message,
            span: None,
            code: None,
        }
    }

    pub(crate) fn coded(code: crate::frontend::diagnostic::Code, message: String) -> Self {
        Self {
            code: Some(code),
            ..Self::new(message)
        }
    }

    fn at(mut self, span: Option<Span>) -> Self {
        if self.span.is_none() {
            self.span = span;
        }
        self
    }

    pub fn render(&self, source: &str) -> String {
        match self.span {
            Some(span) => span.render(source, &self.headed()),
            None => self.to_string(),
        }
    }

    /// The message under its heading: `link error: …`, or a coded
    /// message's own `error[E-…]: …` (`diagnostic::Code`).
    pub fn headed(&self) -> String {
        match self.code {
            Some(c) => c.message(&self.message),
            None => format!("link error: {}", self.message),
        }
    }
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(span) = self.span {
            write!(f, "{}:{}: ", span.line, span.col)?;
        }
        write!(f, "{}", self.headed())
    }
}

impl std::error::Error for LinkError {}

pub(crate) type LResult<T> = Result<T, LinkError>;

pub use crate::ir::{
    BlockId, CArg, CArrival, CEvict, CExpr, CPool, CRef, CServe, CSpill, CStage, CStageKind, CStep,
    CStmt, CtxVar, DistKind, Fun, IR_VERSION, SessionInit,
};

/// The linked program is the IR (`crate::ir::Program`); the old name stays.
pub type Linked = crate::ir::Program;

/// Values for a program's declared inputs and replacement expression `def`s: the
/// CLI's `--name value` and `--def`, pyserq's `sets=` and `defs=`.
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
    /// Supply declared input `name` with the constant expression `expr`.
    pub fn set(&mut self, name: &str, expr: &str) -> Result<(), String> {
        crate::frontend::args::check_name(name)?;
        let e = crate::frontend::parser::parse_expr(expr).map_err(|e| {
            format!("invalid expression for program argument `{name} = {expr}`: {e}")
        })?;
        self.lets.push((name.to_string(), e));
        Ok(())
    }

    /// The expression definition `name` has the body
    /// `expr`, which may draw, read attributes and use the definitions
    /// before it, as the program's own body could.
    pub fn define(&mut self, name: &str, expr: &str) -> Result<(), String> {
        check_override_name("def", name)?;
        crate::frontend::parser::parse_expr(expr).map_err(|e| {
            format!("invalid expression in the `def` override `{name} = {expr}`: {e}")
        })?;
        self.defs.retain(|(n, _)| n != name);
        self.defs.push((name.to_string(), expr.to_string()));
        Ok(())
    }

    /// `--instance FILE`, whose text is `src`: each `let` of the instance is
    /// a value of that declared input, and each option of its `run` block the
    /// flag of the same name, in the order they are written, so a later
    /// input or flag wins over the instance as it would over an earlier
    /// one. An instance changes values, never structure, so the program it
    /// gives is one supplied inputs could give, and the IR does not know it.
    pub fn instance(&mut self, src: &str) -> Result<(), String> {
        let (lets, run) =
            crate::frontend::parser::parse_instance(src).map_err(|e| e.render(src))?;
        self.lets.extend(lets);
        let number = |key: &str, e: Option<Expr>| -> Result<Option<f64>, String> {
            let Some(mut e) = e else { return Ok(None) };
            while let Expr::Located(_, inner) = e {
                e = *inner;
            }
            match e {
                Expr::Num(x) => Ok(Some(x)),
                _ => Err(format!(
                    "the instance's run option `{key}` is not a number\nhelp: write the value \
                     (`{key} 2000;`): an instance's run block is what the run flags would say"
                )),
            }
        };
        if let Some(x) = number("horizon", run.horizon)? {
            if !(x.is_finite() && x > 0.0) {
                return Err(format!(
                    "the instance's horizon {x} is not a finite positive number"
                ));
            }
            self.horizon = Some(x);
        }
        if let Some(x) = number("warmup", run.warmup)? {
            if !(x.is_finite() && x >= 0.0) {
                return Err(format!(
                    "the instance's warmup {x} is not a finite nonnegative number"
                ));
            }
            self.warmup = Some(x);
        }
        if let Some(x) = number("seed", run.seed)? {
            if !(x >= 0.0 && x.fract() == 0.0 && x <= u64::MAX as f64) {
                return Err(format!(
                    "the instance's seed {x} is not an unsigned integer"
                ));
            }
            self.seed = Some(x as u64);
        }
        if let Some(x) = number("arrivals", run.arrivals)? {
            if !(x >= 1.0 && x.fract() == 0.0 && x <= usize::MAX as f64) {
                return Err(format!(
                    "the instance's arrivals {x} is not a positive integer"
                ));
            }
            self.arrivals = Some(x as usize);
        }
        Ok(())
    }

    /// Supply declared input `name` with `x`, exactly (no text round trip).
    /// An infinity is `inf`, as `--name inf` writes it; NaN is refused
    /// when the program is linked, as any constant that is NaN.
    pub fn set_num(&mut self, name: &str, x: f64) -> Result<(), String> {
        crate::frontend::args::check_name(name)?;
        self.lets.push((name.to_string(), Expr::Num(x)));
        Ok(())
    }
}

/// An override's name is an identifier; `kind` is what it overrides.
fn check_override_name(kind: &str, name: &str) -> Result<(), String> {
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
        Err(format!(
            "invalid `{kind}` override name `{name}`; expected an identifier"
        ))
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
    sides: Vec<Vec<crate::ir::Side>>,
    declarations: Vec<(usize, DeclaredType)>,
    side: crate::ir::Side,
    /// Per block, per statement: where the statement is in the text, as
    /// far as a reference or an expression in it says (#279), and where its
    /// own expressions read each name (#531).
    spans: Spans,
    prog: &'a Program,
    /// The step stages' registers (`state`), and their indices by name.
    registers: Vec<crate::ir::Register>,
    /// The gateways' routes, where the blocks put them (`Program::routes`).
    routes: Vec<crate::ir::Route>,
    reg_index: HashMap<String, usize>,
    /// Terms the program's aggregates have written out so far, nested ones
    /// included, against `MAX_OVER`.
    terms_written: Cell<usize>,
    /// While a family engine's admission is read, its size: a family of as
    /// many members named without an index is the member that admits
    /// (`request_member` writes it).
    member_of: Cell<Option<usize>>,
    /// Which side reads the expression being linked, under a server's
    /// signature: a statement's side, the server for the scheduler's
    /// declarations, and neither for a gauge or a claim.
    reader: Cell<Option<crate::ir::Side>>,
}

/// The context variables by their source names (`docs/api/context.md`). A
/// name resolves to an attribute first, then a `let`, then one of these, so
/// neither an attribute nor a `let` may take one of these names (`link`
/// rejects it, as it does a `let` and an attribute of one name): the
/// expression that meant the context variable would read the attribute
/// instead (#231).
pub const CONTEXT_VARS: [(&str, CtxVar); 30] = [
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
    ("position", CtxVar::Position),
    ("demand", CtxVar::Demand),
    ("served", CtxVar::Served),
    ("arrived", CtxVar::Arrived),
    ("admitted", CtxVar::Admitted),
    ("preempted", CtxVar::Preempted),
    // an engine's request, read as `request.…` (no bare name, so no
    // attribute is named like one)
    ("request.input", CtxVar::Input),
    ("request.output", CtxVar::Output),
    ("request.known", CtxVar::Known),
    ("request.hit", CtxVar::Hit),
    ("request.finished", CtxVar::Finished),
    ("request.loaded", CtxVar::Loaded),
    ("request.preempted", CtxVar::Requeued),
];

/// What an engine sets on the session of a request, read by its caller
/// after the call as `E.admitted` and `E.first`: when the request was last
/// admitted and when its first token was sampled (`ir::RequestRecords`).
pub(crate) const REQUEST_ATTRS: [&str; 4] = ["admitted", "first", "reused", "prefill"];

/// What an engine's connector sets on the session of a request, read by
/// its caller after the call as `E.loaded` and `E.saved`: the tokens its
/// latest admission loaded, and the tokens the L2 holds of it after its
/// save (`ir::CConnector`).
pub(crate) const CONNECTOR_ATTRS: [&str; 3] = ["loaded", "saved", "load_done"];

/// The attribute a queue's entry sets, before it calls its engine, to the
/// member of its `from` source, which a KV relation's connector reads from
/// (`E.from`).
pub(crate) const RELATION_FROM: &str = "from";

/// The names an engine reads its values under: `running.count`,
/// `waiting.count`, `batch.tokens` (`parser/device.rs`).
pub(crate) const LISTS: [&str; 4] = ["running", "waiting", "batch", "request"];

/// What the language supplies under a name read as a value: a context
/// variable, or `inf`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Supplied {
    Ctx(CtxVar),
    Inf,
}

/// The name a program reads infinity by.
pub const INF: &str = "inf";

/// Whether the language supplies `name` as a value, and as what: the one
/// place `CONTEXT_VARS` and `inf` are read together. An engine's lists
/// (`LISTS`) are the language's too, but read as `name.…`, not as a value.
pub(crate) fn supplied(name: &str) -> Option<Supplied> {
    if name == INF {
        return Some(Supplied::Inf);
    }
    CONTEXT_VARS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| Supplied::Ctx(*v))
}

/// What a bare name read as a value means, in the order the linker
/// resolves it: an attribute, a `let` constant, an engine's register, then
/// what the language supplies (#538).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Meaning {
    Attr(usize),
    Const(f64),
    Reg(usize),
    Supplied(Supplied),
}

/// Where the language reads `name`, when it supplies it: a context
/// variable at its moments, `inf` in every expression, an engine's list in
/// the engine.
fn read_in(name: &str) -> Option<String> {
    let moments = match supplied(name) {
        Some(Supplied::Inf) => &[][..],
        Some(Supplied::Ctx(var)) => var.moments(),
        // `self` is a queue's entry's: the member's index, read before any
        // parameter or local of that name (`queue::in_entry`)
        None if name == crate::frontend::queue::SELF => {
            return Some("a queue's entry, as its member's index".into());
        }
        None => {
            return LISTS
                .contains(&name)
                .then(|| format!("an engine, as `{name}.…`"));
        }
    };
    Some(match moments {
        [] => "every expression".to_string(),
        ms => ms
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join(", or "),
    })
}

/// The refusal of `kind` (`a session attribute`, `a stage`, …) named
/// `name`, when the language supplies that name. A program may not name its
/// own thing so, an attribute or a `let` (`link`, #231) or a declaration or
/// a parameter (`parser.rs`, #460): the one word would mean two things.
pub(crate) fn takes_supplied(name: &str, kind: &str) -> Option<String> {
    let read_in = read_in(name)?;
    Some(format!(
        "`{name}` is a name the language supplies, read in {read_in}; {kind} named \
         `{name}` would make the one word mean two things\n\
         help: name it otherwise (docs/api/context.md lists the names the language supplies)"
    ))
}

/// The most terms the aggregates (`max j in n (e)`) of one program write
/// out, nested ones included.
pub(crate) const MAX_OVER: usize = 4096;

/// Calls lowered directly by the linker instead of a numeric `Fun`: a
/// declaration query (`blocksize`) and a resource conversion (`cost`).
pub const FOLDED: [&str; 2] = ["blocksize", "cost"];

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
pub(crate) const RENAMED: [(&str, &str); 9] = [
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

/// The built-in attributes the scheduler keeps of a request: its cache
/// hit, which a hold's admission sets, and the tokens it has computed,
/// which a preemption sets. A queue entry reads them without `hidden`; the
/// session's own (`out`, `more`, …) it reads only if hidden.
pub const SCHEDULER_ATTRS: [&str; 2] = ["cached", "computed"];

/// The session attribute a queue entry's `prefix` key reads without
/// `hidden`: the session's number, by which a request keys its own context
/// apart from the segments it shares (`prefix (system: S, serial: inf)`).
pub const KEY_ATTRS: [&str; 1] = ["serial"];

pub fn link(prog: &Program, ov: &Overrides) -> LResult<Linked> {
    link_located(prog, ov).map(|(p, _)| p)
}

/// Where a linked statement is in the text: the statement, and each name
/// its own expressions read (its header's, not its body's), at its first
/// reading, so that an error about one of them points at it (#531).
#[derive(Clone, Debug, Default)]
pub struct StmtSpan {
    pub at: Option<Span>,
    pub names: Vec<(String, Span)>,
}

impl StmtSpan {
    /// Where an error about this statement points: the name its message
    /// quotes first, when the statement reads it, else the statement. It
    /// reads `Program::validate`'s wording, which quotes the offending name
    /// first; a message that quotes something else falls back to the
    /// statement.
    pub fn of(&self, message: &str) -> Option<Span> {
        let quoted = message.split('`').nth(1);
        quoted
            .and_then(|q| self.names.iter().find(|(n, _)| n == q))
            .map(|(_, s)| *s)
            .or(self.at)
    }
}

/// Where each linked statement is in the text: per block, per statement.
pub type Spans = Vec<Vec<StmtSpan>>;

/// `link`, and where each statement of the IR came from, so that an error
/// `Program::validate` finds in a statement can point at the text.
pub fn link_located(prog: &Program, ov: &Overrides) -> LResult<(Linked, Spans)> {
    if !prog.has_main {
        return Err(LinkError::new(
            "missing `fn main()`: definitions do not execute by themselves".into(),
        ));
    }
    for (name, _) in &ov.lets {
        if !prog.inputs.iter().any(|(declared, _)| declared == name) {
            let names: Vec<_> = prog.inputs.iter().map(|(n, _)| n.as_str()).collect();
            return Err(LinkError::new(format!(
                "unknown program argument `{name}`\nhelp: only inputs declared with `args.number` can be supplied; available arguments: {}",
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
        sides: vec![],
        declarations: vec![],
        side: crate::ir::Side::Workload,
        spans: vec![],
        prog,
        terms_written: Cell::new(0),
        member_of: Cell::new(None),
        reader: Cell::new(None),
        registers: vec![],
        reg_index: HashMap::new(),
        routes: vec![],
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
    let declared = lk.attrs.iter().map(|n| (n, "a session attribute"));
    let declared = declared.chain(prog.lets.iter().map(|(n, _)| (n, "a `let` constant")));
    for (name, kind) in declared {
        if let Some(refusal) = takes_supplied(name, kind) {
            return Err(LinkError::new(refusal));
        }
    }
    // Constants, in order; supplied inputs replace only their declared defaults.
    for (index, (name, e)) in prog.lets.iter().enumerate() {
        let input = prog
            .inputs
            .iter()
            .find(|(_, declaration)| *declaration == index)
            .map(|(key, _)| key);
        let overridden = ov.lets.iter().any(|(n, _)| Some(n) == input);
        let e = ov
            .lets
            .iter()
            .rev()
            .find(|(n, _)| Some(n) == input)
            .map(|(_, e)| e)
            .unwrap_or(e);
        let what = if overridden {
            "the value".to_string()
        } else {
            format!("`let {name}`")
        };
        let v = lk.eval_const_named(e, &what).map_err(|mut error| {
            if overridden {
                // These spans refer to the override's expression, not the program.
                error.message = format!(
                    "the program argument `{}`: {}",
                    input.unwrap_or(name),
                    error.message
                );
                error.span = None;
            }
            error
        })?;
        lk.consts.insert(name.clone(), v);
    }
    // An array size or a relation's `ttl` is folded by the parser, which
    // knew only the `let`s above it, and reaches here as a number: its
    // aggregates' indices are asked of the whole program, as an
    // expression's are (#561).
    for e in &prog.folded {
        lk.own_indices(e)?;
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
    // Registers: a step stage's `state`, before anything that may read one.
    // A register's name is its own: not an attribute, a constant, a name
    // the language supplies, a pool or stage, or another register.
    let mut stage_base = 0;
    for s in &prog.stages {
        if let StageKind::Step(sp) = &s.kind {
            for (name, init) in &sp.state {
                if s.count != 1 {
                    return Err(LinkError::new(format!(
                        "engine `{}`: `state {name}` on an engine family; which member's register \
                         an expression read would be a guess",
                        s.name
                    )));
                }
                let taken = lk.resolve(name).is_some()
                    || lk.pools.contains_key(name)
                    || lk.stages.contains_key(name);
                if taken {
                    return Err(LinkError::new(format!(
                        "engine `{}`: `state {name}`: the name is taken; a register's name is \
                         its own",
                        s.name
                    )));
                }
                let init = lk.eval_const_named(init, &format!("`state {name}`"))?;
                lk.reg_index.insert(name.clone(), lk.registers.len());
                lk.registers.push(crate::ir::Register {
                    name: name.clone(),
                    stage: stage_base,
                    init,
                });
            }
        }
        stage_base += s.count;
    }
    // Pools and stages are the scheduler's: they read what the server
    // reads.
    lk.reader.set(Some(crate::ir::Side::Server));
    // Pools.
    let mut pools = vec![];
    for p in &prog.pools {
        let cap = lk.eval_const_named(&p.cap, &format!("pool `{}`: cap", p.name))?;
        let block = p
            .block
            .as_ref()
            .map(|b| lk.eval_const_named(b, &format!("pool `{}`: block", p.name)))
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
        let preempt = match &p.preempt {
            PreemptOrder::None => crate::ir::Preempt::None,
            PreemptOrder::By { keys, tail } => crate::ir::Preempt::By {
                keys: keys.iter().map(|k| lk.expr(k)).collect::<LResult<_>>()?,
                tail: *tail,
            },
            PreemptOrder::Leases {
                keys,
                every,
                suspend,
            } => crate::ir::Preempt::Leases {
                keys: keys.iter().map(|k| lk.expr(k)).collect::<LResult<_>>()?,
                every: every
                    .as_ref()
                    .map(|e| lk.eval_const_named(e, &format!("pool `{}`: every", p.name)))
                    .transpose()?,
                suspend: suspend
                    .as_ref()
                    .map(|ks| ks.iter().map(|k| lk.expr(k)).collect::<LResult<_>>())
                    .transpose()?,
            },
        };
        let queue = match &p.queue {
            QueueOrder::Fifo => None,
            QueueOrder::By(keys) => Some(keys.iter().map(|k| lk.expr(k)).collect::<LResult<_>>()?),
        };
        let queue_every = p
            .queue_every
            .as_ref()
            .map(|e| lk.eval_const_named(e, &format!("pool `{}`: queue every", p.name)))
            .transpose()?;
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
            // a queue family's `pool q on S`: q[i] is served by S[i]
            let admit_via = admit_via
                .map(|(b, c)| member(b, c, i, p.count, &p.name, "on"))
                .transpose()?;
            pools.push(CPool {
                admit_via,
                name: p.name.clone(),
                index: p.array.then_some(i as u32),
                cap,
                block,
                evict: evict.clone(),
                preempt: preempt.clone(),
                queue: queue.clone(),
                queue_every,
                spill: spill.clone(),
                reserve_held: p.reserve_held,
            });
        }
    }
    // Stages.
    let mut stages = vec![];
    for s in &prog.stages {
        let kind = match &s.kind {
            StageKind::Fifo(c) => {
                let c =
                    lk.eval_const_named(c, &format!("stage `{}`: fifo server count", s.name))?;
                if c < 1.0 || c.fract() != 0.0 {
                    return Err(LinkError::new(format!(
                        "stage `{}`: fifo servers must be a positive integer",
                        s.name
                    )));
                }
                CStageKind::Fifo(c as usize)
            }
            StageKind::Ps(phi) => CStageKind::Ps(lk.expr(phi)?),
            StageKind::Delay => CStageKind::delay(),
            StageKind::Step(sp) => CStageKind::Step(CStep {
                budget: lk.expr(&sp.budget)?,
                cost: lk.expr(&sp.cost)?,
                chunk: lk.per_run_cap(&sp.chunk, &s.name, None)?,
                granule: match &sp.granule {
                    None => None,
                    Some(g) => {
                        let v = lk.eval_const_named(g, &format!("engine `{}`: granule", s.name))?;
                        if v.is_nan() || v <= 0.0 {
                            return Err(LinkError::new(format!(
                                "engine `{}`: granule {v}: a prefill takes a multiple of it, \
                                 so it is above 0 (`inf` for whole or nothing)",
                                s.name
                            )));
                        }
                        Some(CExpr::Num(v))
                    }
                },
                serve: serve(&lk, &sp.serve)?,
                request: sp
                    .request
                    .as_ref()
                    .map(|r| lk.request(r, &s.name).map(Box::new))
                    .transpose()?,
                // its records are attributes now, so that its caller reads
                // them; its blocks are written after the session's, below
                connector: {
                    if sp.connector.is_some() {
                        for a in CONNECTOR_ATTRS {
                            lk.attr(&format!("{}.{a}", s.name));
                        }
                    }
                    None
                },
                memory: sp.memory.as_ref().map(|m| lk.pool_base(m)).transpose()?,
                iteration: match &sp.schedule {
                    Schedule::Body(b) => Some(iteration(&lk, lk.stages[&s.name].0, b)?),
                    // vLLM's procedure sets no register; one the program
                    // declares would stay at its first value
                    Schedule::Procedure(_) if !sp.state.is_empty() => {
                        return Err(LinkError::new(format!(
                            "engine `{}`: register `{}` is set by nothing: a schedule that \
                             sets it is written with `set {} = …;`",
                            s.name, sp.state[0].0, sp.state[0].0
                        )));
                    }
                    Schedule::Procedure(None) => None,
                    // `only (p)` shared by `advance running` and `admit
                    // waiting` is the body that serves only `p` and admits
                    // while the iteration has not preempted, each newcomer
                    // `p` excludes waiting unserved (#355)
                    Schedule::Procedure(Some(p)) => {
                        let p = lk.expr(p)?;
                        Some(vec![
                            CIter::Serve {
                                only: Some(p.clone()),
                                by: None,
                            },
                            CIter::Admit {
                                only: Some(p),
                                gate: Some(CExpr::Unary(
                                    UnOp::Not,
                                    Box::new(CExpr::Ctx(CtxVar::Preempted)),
                                )),
                            },
                        ])
                    }
                },
            }),
        };
        let memory = match &s.kind {
            StageKind::Step(sp) => sp.memory.as_ref().map(|m| lk.pool_span(m)).transpose()?,
            _ => None,
        };
        for i in 0..s.count {
            let mut kind = kind.clone();
            // an engine family `E[N]` whose memory is `kv[N]`: E[i]'s memory
            // is kv[i]; with one pool, every E[i]'s is it
            if let (CStageKind::Step(st), Some((b, c))) = (&mut kind, memory) {
                st.memory = Some(member(b, c, i, s.count, &s.name, "memory")?);
            }
            // and E[i]'s request takes E[i]'s pools: a family the engine's
            // size, written without an index, is its member
            if let CStageKind::Step(st) = &mut kind
                && let Some(r) = &mut st.request
                && s.count > 1
            {
                request_member(r, i, s.count, &s.name)?;
            }
            stages.push(CStage {
                name: s.name.clone(),
                index: s.array.then_some(i as u32),
                kind,
            });
        }
    }
    lk.reader.set(None);
    // Workload.
    let (arrival, trace, trace_ordered, init, turn) = match wl {
        None => (CArrival::None, None, false, vec![], vec![]),
        Some(w) => {
            let a = match &w.arrive {
                Arrival::Poisson(e) => {
                    CArrival::Poisson(lk.eval_const_named(e, "the poisson rate")?)
                }
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
    let horizon = ov.horizon.ok_or_else(|| {
        LinkError::new(
            "no horizon: supply --horizon T, --instance FILE, or Overrides.horizon".into(),
        )
    })?;
    let warmup = ov.warmup.unwrap_or(0.0);
    let seed = ov.seed.unwrap_or(1);
    let arrivals = ov.arrivals;
    if warmup >= horizon {
        return Err(LinkError::new("warmup must be below the horizon".into()));
    }
    // the hidden parameters, which the scheduler's expressions may not
    // read; `Program::validate` enforces it by moment and rejects what the
    // scheduler itself sets. The parser sends every parameter, so each is an
    // attribute (`lower_send`)
    let mut hidden = vec![];
    if let Some(w) = prog.workload.as_ref() {
        for n in &w.hidden {
            hidden.push(lk.attr_index[n]);
        }
    }
    if let Some(w) = prog.workload.as_ref() {
        hidden_in_server(
            &w.init,
            &w.turn,
            &prog.session,
            &prog.server,
            &w.hidden,
            &scheduler_reads(prog),
        )?;
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
    // an engine's connector: its transfers, lowered after the session's
    // blocks, which keep their numbers; a family's, one per member
    let mut sends: Vec<(String, Option<f64>)> = vec![];
    let mut at = 0;
    for decl in &prog.stages {
        for i in 0..decl.count {
            let st = &mut stages[at];
            at += 1;
            if let (StageKind::Step(sp), CStageKind::Step(step)) = (&decl.kind, &mut st.kind)
                && let Some(c) = &sp.connector
            {
                let taken = sp.memory.as_ref().filter(|m| {
                    sp.request
                        .as_ref()
                        .is_some_and(|r| r.pools.iter().any(|(q, _, _)| q.name == m.name))
                });
                if c.load.is_some() && taken.is_none() {
                    return Err(connector_untaken(&decl.name, sp).at(decl.span));
                }
                let member = decl.array.then_some(i);
                step.connector = Some(Box::new(lk.connector(c, &decl.name, taken, member)?));
                if let ConnectorKind::Relation(Some(rel)) = &c.kind {
                    sends.push((rel.source.clone(), rel.ttl));
                }
            }
        }
    }
    // a KV relation's source engine registers a send of each request's
    // blocks, which the reader's load ends, or its `ttl`: an engine that
    // takes its memory (a source whose entries hold and lease keeps the
    // lease they write)
    for &(ref source, ttl) in &sends {
        for st in stages.iter_mut().filter(|st| st.name == *source) {
            if let CStageKind::Step(step) = &mut st.kind
                && let Some(m) = step.memory
                && let Some(r) = &mut step.request
                && r.pools.iter().any(|(q, _, _)| q.base == m)
            {
                // two relations from one source name one `ttl`: the parser
                // refused the others, where the relation is written
                r.send = Some(crate::ir::CSend { ttl });
            }
        }
    }
    // what an engine records of its requests, after the program's own
    // observations, which keep their places
    for st in &mut stages {
        if let CStageKind::Step(step) = &mut st.kind
            && let Some(r) = &mut step.request
        {
            let name = |what: &str| format!("{}.{what}", st.name);
            r.records = crate::ir::RequestRecords {
                reused: lk.observe_slot(&name("reused")),
                prefill: lk.observe_slot(&name("prefill")),
                ..r.records
            };
        }
    }
    let slot = |lk: &Linker, n: &str| lk.attr_index[n];
    let mut linked = Linked {
        gauges,
        claims,
        registers: lk.registers.clone(),
        routes: std::mem::take(&mut lk.routes),
        transfer_blocks: None,
        version: IR_VERSION,
        hidden,
        slot_cached: slot(&lk, "cached"),
        slot_serial: slot(&lk, "serial"),
        slot_turn: slot(&lk, "turn_no"),
        slot_new: slot(&lk, "new"),
        slot_out: slot(&lk, "out"),
        slot_think: slot(&lk, "think"),
        slot_more: slot(&lk, "more"),
        slot_forced: slot(&lk, "forced"),
        slot_computed: slot(&lk, "computed"),
        attr_types: vec![],
        sides: lk.sides,
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
    for (name, _) in &prog.cost_records {
        if linked.attrs.iter().any(|n| n == name)
            || linked.pools.iter().any(|p| p.name == *name)
            || linked.stages.iter().any(|s| s.name == *name)
            || prog.lets.iter().any(|(n, _)| n == name)
        {
            return Err(LinkError::new(format!(
                "Cost record `{name}` conflicts with a scalar attribute, constant, or resource; give it a separate name"
            )));
        }
    }
    linked.infer_types().map_err(LinkError::new)?;
    for (slot, declared) in lk.declarations {
        let actual = &linked.attr_types[slot];
        let matches = match declared {
            DeclaredType::Size => *actual == crate::ir::ValueType::Size,
            DeclaredType::Cost => matches!(actual, crate::ir::ValueType::Cost(_)),
        };
        if !matches {
            return Err(LinkError::new(format!(
                "`{}` is declared {declared:?} but its assignments do not have that type; a Cost needs a named resource conversion",
                linked.attrs[slot]
            )));
        }
    }
    crate::frontend::lint::lint(&linked).map_err(LinkError::new)?;
    Ok((linked, lk.spans))
}

/// A connector's lowered statements as its transfer (`ir::CTransfer`):
/// `blocks[0]` is `run; load; [release S]` for a load, `hold P { grow; run;
/// load } cache` for a save (its body `blocks[1]`). The lowering above writes
/// only these shapes.
fn transfer(amount: CExpr, blocks: Vec<Vec<CStmt>>) -> crate::ir::CTransfer {
    let (stmts, room) = match &blocks[0][..] {
        [CStmt::Hold { pools, cache, .. }] => {
            let body = &blocks[1];
            let [CStmt::Grow(_, grow), ..] = &body[..] else {
                unreachable!("a save grows its room first")
            };
            let room = crate::ir::CRoom {
                held: pools[0].1.clone(),
                grow: grow.clone(),
                cache: cache.clone().expect("a save caches"),
            };
            (body[1..].to_vec(), Some(room))
        }
        b => (b.to_vec(), None),
    };
    let mut source = None;
    let mut run = None;
    let mut load = None;
    for st in stmts {
        match st {
            CStmt::Run {
                stage,
                work,
                also,
                share,
                latency,
                poster,
                ..
            } => run = Some((stage, work, also, share, latency, poster)),
            CStmt::Load(r, e) => load = Some((r, e)),
            CStmt::Release(r) => source = Some(r),
            s => unreachable!("a connector's lowering writes no {s:?}"),
        }
    }
    let (stage, work, also, share, latency, poster) = run.expect("a transfer copies");
    let (into, units) = load.expect("a transfer lands");
    crate::ir::CTransfer {
        amount,
        stage,
        also,
        work,
        share,
        latency,
        poster,
        into,
        units,
        room,
        source,
    }
}

/// The refusal of an engine whose connector loads into its memory, the KV
/// pool on its device, which its `taking` does not take: the load would
/// grow a hold the request does not have.
fn connector_untaken(engine: &str, sp: &StepSpec) -> LinkError {
    use crate::frontend::diagnostic::Code;
    let message = format!(
        "engine `{engine}`'s connector loads into the engine's memory, the pool on its device, \
         which its `taking` does not take"
    );
    // the pool as the queue's text names its own (`kv`, not `D.kv`); how
    // much a request takes of it is the program's to say (criterion 2)
    let pool = |m: &Ref| {
        m.name
            .strip_prefix(&format!("{engine}."))
            .unwrap_or(&m.name)
            .to_string()
    };
    let help = match (&sp.memory, &sp.request) {
        (Some(m), Some(_)) => format!(
            "take `{}` in the `taking` of `{engine}`'s `admit waiting`: `{} (…)`, what a \
             request holds of it once its KV is loaded",
            pool(m),
            pool(m)
        ),
        (Some(m), None) => format!(
            "take `{}` in `{engine}`'s `admit waiting`: `taking {} (…)`, what a request holds \
             of it once its KV is loaded",
            pool(m),
            pool(m)
        ),
        (None, _) => format!(
            "put a KV pool on `{engine}`'s device (`pool kv on gpu {{ block 16; }}`) and take it \
             in `admit waiting`"
        ),
    };
    LinkError::coded(Code::ConnectorUntaken, format!("{message}\nhelp: {help}"))
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
            Expr::Unread(_, x) => expr_span(x),
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
        Stmt::Grow(r, e) | Stmt::Shrink(r, e) | Stmt::Load(r, e) => r.span.or_else(|| expr_span(e)),
        Stmt::Drop(r) | Stmt::Release(r) => r.span,
        Stmt::Run { stage, work, .. } => stage.span.or_else(|| expr_span(work)),
        Stmt::Set(_, e) | Stmt::Observe(_, e) | Stmt::Branch(e, _, _) => expr_span(e),
        Stmt::Choose { count, .. } => expr_span(count),
        _ => None,
    }
}

/// The names a statement's own expressions read, each at its first
/// reading: a hold's header (units, `reserve`, `reuse`, `cache`, `lease`),
/// a run's work, a `set`'s value, a condition; not a body's statements,
/// which have their own.
fn stmt_names(s: &Stmt) -> Vec<(String, Span)> {
    fn walk(e: &Expr, out: &mut Vec<(String, Span)>) {
        match e {
            // a name where it is written: a parameter an entry substituted
            // keeps its own place around the caller's argument, which is
            // the outermost
            Expr::Located(span, inner) => {
                let mut read = inner.as_ref();
                while let Expr::Located(_, i) = read {
                    read = i;
                }
                if let Expr::Var(n) = read
                    && !out.iter().any(|(m, _)| m == n)
                {
                    out.push((n.clone(), *span));
                }
                walk(inner, out);
            }
            Expr::Num(_) | Expr::Var(_) => {}
            Expr::Unary(_, a) => walk(a, out),
            Expr::Binary(_, a, b) | Expr::Over(_, _, a, b) => {
                walk(a, out);
                walk(b, out);
            }
            Expr::Cond(c, a, b) => {
                walk(c, out);
                walk(a, out);
                walk(b, out);
            }
            Expr::Sample(_, xs) => xs.iter().for_each(|x| walk(x, out)),
            Expr::Unread(_, x) => walk(x, out),
            Expr::Call(_, args) => args.iter().for_each(|a| {
                if let Arg::Expr(x) = a {
                    walk(x, out);
                }
            }),
        }
    }
    let mut out = vec![];
    let exprs: Vec<&Expr> = match s {
        Stmt::Hold {
            pools,
            reuse,
            cache,
            lease,
            ..
        } => pools
            .iter()
            .flat_map(|(_, u, r)| std::iter::once(u).chain(r))
            .chain(reuse)
            .chain(cache)
            .chain(lease.as_ref().map(|(_, t)| t))
            .collect(),
        Stmt::Grow(_, e) | Stmt::Load(_, e) | Stmt::Set(_, e) | Stmt::Observe(_, e) => vec![e],
        Stmt::Branch(e, _, _) | Stmt::While(e, _) => vec![e],
        Stmt::Run { work, latency, .. } => std::iter::once(work).chain(latency).collect(),
        Stmt::Choose { count, key, .. } => std::iter::once(count).chain(key).collect(),
        _ => vec![],
    };
    for e in exprs {
        walk(e, &mut out);
    }
    out
}

/// A step stage's serving order. `admission` has no keys: every resident
/// ties, and ties are admission order; `decode first` is `by (decoding ? 0
/// : 1)`, so the IR knows one form.
fn serve(lk: &Linker, s: &Serve) -> LResult<CServe> {
    Ok(match s {
        Serve::Admission => CServe::By(vec![]),
        Serve::DecodeFirst => CServe::By(vec![CExpr::Cond(
            Box::new(CExpr::Ctx(CtxVar::Decoding)),
            Box::new(CExpr::Num(0.0)),
            Box::new(CExpr::Num(1.0)),
        )]),
        Serve::By(keys) => CServe::By(keys.iter().map(|k| lk.expr(k)).collect::<Result<_, _>>()?),
        Serve::ExclusivePrefill => CServe::ExclusivePrefill,
    })
}

/// A step stage's `iteration` body.
fn iteration(lk: &Linker, stage: usize, body: &[IterStmt]) -> LResult<Vec<CIter>> {
    body.iter()
        .map(|s| {
            Ok(match s {
                IterStmt::Serve { only, order } => CIter::Serve {
                    only: only.as_ref().map(|e| lk.expr(e)).transpose()?,
                    by: match order.as_ref().map(|o| serve(lk, o)).transpose()? {
                        None => None,
                        Some(CServe::By(keys)) => Some(keys),
                        Some(CServe::ExclusivePrefill) => {
                            unreachable!("the parser refuses `exclusive prefill` in a body")
                        }
                    },
                },
                IterStmt::Admit { only, gate } => CIter::Admit {
                    only: only.as_ref().map(|e| lk.expr(e)).transpose()?,
                    gate: gate.as_ref().map(|e| lk.expr(e)).transpose()?,
                },
                IterStmt::Branch(g, a, b) => CIter::Branch(
                    lk.expr(g)?,
                    iteration(lk, stage, a)?,
                    iteration(lk, stage, b)?,
                ),
                IterStmt::Set(name, e) => {
                    let here = lk
                        .stages
                        .iter()
                        .find(|&(_, &(b, _))| b == stage)
                        .map_or("?", |(n, _)| n.as_str());
                    let Some(&r) = lk.reg_index.get(name) else {
                        return Err(LinkError::new(format!(
                            "engine `{here}`: `set {name}` in its schedule: not one of its \
                             registers; a schedule sets a register, not a session attribute \
                             (declare it with `state {name} = …;`)"
                        )));
                    };
                    if lk.registers[r].stage != stage {
                        return Err(LinkError::new(format!(
                            "engine `{here}`: `set {name}` in its schedule: the register is \
                             another engine's; a \
                             schedule sets its own engine's"
                        )));
                    }
                    CIter::Set(r, lk.expr(e)?)
                }
            })
        })
        .collect()
}

/// An expression a pool or an engine reads of one session, at its moment:
/// a key, an `only`, a spill clause.
struct Key<'a> {
    /// "pool `P`" or "engine `E`", as `Program::validate` names it.
    owner: String,
    /// The declaration, where the key has no location of its own.
    at: Option<Span>,
    moment: Moment,
    expr: &'a Expr,
}

/// What an engine reads of a request while it is the engine's.
struct EngineReads<'a> {
    /// `taking`, `reuse`, `prefix` and the connector's `load`, read when the
    /// scheduler admits the request (`Moment::Request`).
    admission: Vec<&'a Expr>,
    /// `advance running by (…)` keys and every `only`, read for each
    /// running request (`Moment::Serve`).
    serve: Vec<Key<'a>>,
    /// The pools the request waits in or holds: those it takes, the
    /// engine's memory and the queues the engine admits.
    pools: BTreeSet<String>,
}

/// What the scheduler reads of a session outside the server's statements,
/// by where the session is when it is read (`hidden_in_server`).
#[derive(Default)]
struct SchedulerReads<'a> {
    /// A pool's `queue by` keys: read while the session waits in its queue.
    select: BTreeMap<String, Vec<Key<'a>>>,
    /// A pool's `preempt by` keys: read while the session holds the pool.
    /// A pool here preempts its holder, whose hold `Interp::preempt` puts
    /// back in a queue with its header evaluated again.
    victim: BTreeMap<String, Vec<Key<'a>>>,
    /// Of those, the pools that put it back as a newcomer (`requeue tail`),
    /// whose queue's keys read the session as it was when preempted. With
    /// `requeue head` it goes back ahead of the keys, which never read it
    /// (`Interp::next_waiter_among`).
    requeue: BTreeSet<String>,
    /// Eviction keys and spill clauses, which read the session that wrote
    /// a cache entry, and `preempt leases by` and `suspend by` keys, which
    /// read the session a lease is of: wherever the session is.
    anywhere: Vec<Key<'a>>,
    engines: BTreeMap<String, EngineReads<'a>>,
}

/// The deployment's pools and engines, as `hidden_in_server` reads them. A
/// paired workload's own pools and stages are the client's, which knows
/// what it hides.
fn scheduler_reads(prog: &Program) -> SchedulerReads<'_> {
    let client = |name: &str| {
        prog.paired
            .as_ref()
            .is_some_and(|p| p.iter().any(|n| n == name))
    };
    let mut reads = SchedulerReads::default();
    for p in prog.pools.iter().filter(|p| !client(&p.name)) {
        let key = |moment, expr| Key {
            owner: format!("pool `{}`", p.name),
            at: p.span,
            moment,
            expr,
        };
        if let QueueOrder::By(keys) = &p.queue {
            let select = keys.iter().map(|e| key(Moment::Select, e)).collect();
            reads.select.insert(p.name.clone(), select);
        }
        match &p.preempt {
            PreemptOrder::None => {}
            PreemptOrder::By { keys, tail } => {
                let victim = keys.iter().map(|e| key(Moment::Victim, e)).collect();
                reads.victim.insert(p.name.clone(), victim);
                if *tail {
                    reads.requeue.insert(p.name.clone());
                }
            }
            PreemptOrder::Leases { keys, suspend, .. } => reads.anywhere.extend(
                keys.iter()
                    .chain(suspend.iter().flatten())
                    .map(|e| key(Moment::Victim, e)),
            ),
        }
        if let EvictOrder::By(keys) = &p.evict {
            reads
                .anywhere
                .extend(keys.iter().map(|e| key(Moment::Evict, e)));
        }
        if let Some(s) = &p.spill {
            reads
                .anywhere
                .extend([&s.work, &s.when].map(|e| key(Moment::Evict, e)));
        }
    }
    for s in prog.stages.iter().filter(|s| !client(&s.name)) {
        let StageKind::Step(sp) = &s.kind else {
            continue;
        };
        let key = |expr| Key {
            owner: format!("engine `{}`", s.name),
            at: s.span,
            moment: Moment::Serve,
            expr,
        };
        let mut serve: Vec<Key> = vec![];
        if let Serve::By(keys) = &sp.serve {
            serve.extend(keys.iter().map(key));
        }
        let mut read = vec![];
        match &sp.schedule {
            Schedule::Procedure(only) => read.extend(only),
            Schedule::Body(stmts) => serve_reads(stmts, &mut read),
        }
        serve.extend(read.into_iter().map(key));
        let r = sp.request.as_ref();
        let admission = r
            .into_iter()
            .flat_map(|r| {
                r.pools
                    .iter()
                    .flat_map(|(_, u, f)| std::iter::once(u).chain(f))
                    .chain(&r.reuse)
                    .chain(r.prefix.iter().flatten().flat_map(|(k, n)| [k, n]))
            })
            .chain(sp.connector.iter().flat_map(|c| &c.load))
            .collect();
        let pools = r
            .into_iter()
            .flat_map(|r| r.pools.iter().map(|(q, _, _)| q.name.clone()))
            .chain(sp.memory.iter().map(|m| m.name.clone()))
            .chain(
                prog.pools
                    .iter()
                    .filter(|p| p.admit_via.as_ref().is_some_and(|v| v.name == s.name))
                    .map(|p| p.name.clone()),
            )
            .collect();
        reads.engines.insert(
            s.name.clone(),
            EngineReads {
                admission,
                serve,
                pools,
            },
        );
    }
    reads
}

/// What a scheduler body reads of each running request: its `only`s and
/// its `advance running by (…)` keys, in both arms of an `if`.
fn serve_reads<'a>(stmts: &'a [IterStmt], out: &mut Vec<&'a Expr>) {
    for st in stmts {
        match st {
            IterStmt::Serve { only, order } => {
                out.extend(only);
                if let Some(Serve::By(keys)) = order {
                    out.extend(keys);
                }
            }
            IterStmt::Admit { only, .. } => out.extend(only),
            IterStmt::Branch(_, a, b) => {
                serve_reads(a, out);
                serve_reads(b, out);
            }
            IterStmt::Set(..) => {}
        }
    }
}

/// A hidden attribute is the target's until a run reveals it: the server
/// may run work by it (the model ends a decode, not the scheduler), cache by
/// it at release, and observe it, but a decision on it before it is
/// revealed is the scheduler reading what it cannot see. A run whose work
/// reads it reveals it when the run ends (the end of a decode is the EOS
/// the scheduler sees), and from there on the server may decide on it.
/// `Program::validate` refuses a hidden read at every moment but `Session`;
/// the server's own session statements are the rest of the scheduler,
/// which the IR does not tell from the workload's, so they are checked
/// here, on the server as the parser expanded it. An attribute the server
/// sets from a hidden one is hidden until each hidden one it was set from
/// is revealed, and so is what the workload sets from one before a request
/// (#547): the workload is walked in order, `init`, then the session with
/// the `turn` block at each `turn`, and what is set from a hidden value at a
/// request reaches the server. A request's end reveals the turn's outputs to
/// the workload, so a session `set` after it is visible
/// (`set K = K + n + o;`) until a hidden attribute is set again. Paths join
/// conservatively: after a branch an attribute is revealed only if both
/// arms reveal it, and a loop's body may not run.
///
/// The pools' and engines' keys are the scheduler too, and `Program::validate`
/// refuses the hidden attribute itself in them; what was set from it is
/// checked here (#554), each key where the scheduler reads the session, on
/// the attributes as they stand there: a pool's `queue by` keys at a `hold`
/// on it, and at a request to an engine that takes it or admits from it;
/// its `preempt by` keys wherever the session waits holding it, and, under
/// `requeue tail`, its `queue by` keys there too, since a victim's hold goes
/// back to the queue as a newcomer; an engine's `advance running by` keys
/// and its `only`s during a request to it, before the request's end reveals
/// its output. An eviction key or a spill clause reads the session that
/// wrote the entry, and a lease's keys the session the lease is of,
/// wherever it is: they are checked wherever the session waits (a run, a
/// queue, a request, a `grow` or `load`), at a release (a hold's end,
/// `release P;`), whose entry keeps the attributes as they are then, at the
/// server's end, and wherever the workload waits (a `tool` run, a hold).
/// A hold's header is read again where a pool that preempts puts the hold
/// back, wherever its body waits, and its lease's time at its end.
fn hidden_in_server(
    init: &[Stmt],
    turn: &[Stmt],
    session: &[Stmt],
    server: &[Stmt],
    hidden: &[String],
    reads: &SchedulerReads,
) -> LResult<()> {
    #[derive(Clone, Default, PartialEq)]
    struct Origins {
        possible: BTreeSet<String>,
        certain: BTreeSet<String>,
        derived: bool,
        /// where it was set from a hidden attribute, for the message
        site: &'static str,
    }
    #[derive(Clone, Default, PartialEq)]
    struct Taint {
        attrs: BTreeMap<String, Origins>,
        // Keep dependencies after revelation: another loop pass still knows
        // what a run by that value reveals, even if a prior pass revealed it.
        revealed: BTreeSet<String>,
    }
    fn names(e: &Expr) -> Vec<String> {
        let (mut vars, mut refs) = (vec![], vec![]);
        crate::frontend::parser::names(e, &mut vars, &mut refs);
        for r in refs {
            if let Some(i) = r.index.as_deref() {
                crate::frontend::parser::names(i, &mut vars, &mut vec![]);
            }
        }
        vars
    }
    /// Origins read on every evaluation path. Conditional arms and the
    /// right side of a short-circuit operator need not be evaluated.
    fn certain(e: &Expr, t: &Taint) -> BTreeSet<String> {
        match e {
            // an unread argument is checked, never evaluated
            Expr::Located(_, e) | Expr::Unary(_, e) | Expr::Unread(_, e) => certain(e, t),
            Expr::Cond(c, a, b) => {
                let mut out = certain(c, t);
                out.extend(certain(a, t).intersection(&certain(b, t)).cloned());
                out
            }
            Expr::Binary(BinOp::And | BinOp::Or, a, _) | Expr::Over(_, _, a, _) => certain(a, t),
            Expr::Binary(_, a, b) => certain(a, t).union(&certain(b, t)).cloned().collect(),
            Expr::Sample(_, xs) => xs.iter().flat_map(|e| certain(e, t)).collect(),
            // Resource annotations are checked but not evaluated. Only the
            // converted quantity can reveal a hidden input when work ends.
            // A bare name reads its attribute only where the function takes
            // a number: `work(E)` reads the stage `E`, not an attribute `E`.
            Expr::Call(f, args) => args
                .iter()
                .enumerate()
                .skip(if f == "cost" {
                    args.len().saturating_sub(1)
                } else {
                    0
                })
                .flat_map(|(k, a)| match a {
                    Arg::Expr(e) => certain(e, t),
                    Arg::Ref(r) => match &r.index {
                        Some(e) => certain(e, t),
                        None if takes_number(f, k, args.len()) => t
                            .attrs
                            .get(&r.name)
                            .map(|s| s.certain.clone())
                            .unwrap_or_default(),
                        None => BTreeSet::new(),
                    },
                })
                .collect(),
            Expr::Var(n) => t
                .attrs
                .get(n)
                .map(|s| s.certain.clone())
                .unwrap_or_default(),
            Expr::Num(_) => BTreeSet::new(),
        }
    }
    /// Possible origins prohibit decisions; only certain origins let a run
    /// reveal an input. The two differ after control-flow paths join.
    fn origins(e: &Expr, t: &Taint, site: &'static str) -> Origins {
        let mut out = Origins {
            derived: true,
            site,
            certain: certain(e, t),
            ..Default::default()
        };
        for n in names(e) {
            if let Some(from) = t.attrs.get(&n) {
                out.possible.extend(from.possible.iter().cloned());
            }
        }
        out
    }
    fn join(a: &mut Taint, b: &Taint) {
        a.revealed.retain(|h| b.revealed.contains(h));
        for (name, from) in &mut a.attrs {
            if let Some(other) = b.attrs.get(name) {
                from.possible.extend(other.possible.iter().cloned());
                from.certain.retain(|h| other.certain.contains(h));
                from.derived |= other.derived;
            } else {
                from.certain.clear();
            }
        }
        for (name, from) in &b.attrs {
            if !a.attrs.contains_key(name) {
                let mut from = from.clone();
                from.certain.clear();
                a.attrs.insert(name.clone(), from);
            }
        }
    }
    fn why(name: &str, from: &Origins, t: &Taint) -> String {
        if !from.derived {
            format!("`{name}` is hidden from the scheduler")
        } else {
            let hidden: Vec<_> = from.possible.difference(&t.revealed).cloned().collect();
            format!(
                "`{name}` is set from the hidden `{}` in {}",
                hidden.join("`, `"),
                from.site
            )
        }
    }
    /// The first name `e` reads that holds a hidden attribute, or what was
    /// set from one, before a run has revealed it.
    fn reads_unrevealed<'t>(e: &Expr, t: &'t Taint) -> Option<(String, &'t Origins)> {
        names(e).into_iter().find_map(|n| {
            let from = t.attrs.get(&n)?;
            (!from.possible.is_subset(&t.revealed)).then_some((n, from))
        })
    }
    /// `t` with only what was set from a hidden attribute: where
    /// `Program::validate` refuses the attribute itself, the check leaves
    /// it to validate.
    fn copies(t: &Taint) -> Taint {
        let mut derived = t.clone();
        derived.attrs.retain(|_, from| from.derived);
        derived
    }
    /// The server's `what` may not read a name `t` holds unrevealed.
    fn check(e: &Expr, what: &str, t: &Taint) -> LResult<()> {
        match reads_unrevealed(e, t) {
            Some((n, from)) => Err(refuse(e, what, &n, from, t)),
            None => Ok(()),
        }
    }
    /// A hold's header, read at its admission: the index that picks each
    /// pool, and the copies its units, `reserve`, `reuse` and `prefix`
    /// read.
    fn header(hold: &Stmt, what: &str, t: &Taint) -> LResult<()> {
        let Stmt::Hold {
            pools,
            reuse,
            prefix,
            ..
        } = hold
        else {
            return Ok(());
        };
        let copies = copies(t);
        for (r, units, reserve) in pools {
            if let Some(i) = index(r) {
                check(i, "choice of a pool", t)?;
            }
            for e in std::iter::once(units).chain(reserve) {
                check(e, what, &copies)?;
            }
        }
        let rest = reuse
            .iter()
            .chain(prefix.iter().flatten().flat_map(|(k, n)| [k, n]));
        for e in rest {
            check(e, what, &copies)?;
        }
        Ok(())
    }
    fn refuse(e: &Expr, what: &str, name: &str, from: &Origins, t: &Taint) -> LinkError {
        let why = why(name, from, t);
        let span = match e {
            Expr::Located(span, _) => Some(*span),
            _ => None,
        };
        LinkError::new(format!(
            "{why}, but the server's {what} reads it before a run reveals it\nhelp: an engine's \
             request may generate a hidden attribute's tokens (`E (n, o);`: the model ends the \
             run), cache by it at release and observe it, and decide on it once that run has \
             ended; before, a decision on it is the scheduler reading what it cannot see"
        ))
        .at(span)
    }
    /// Where a key reads `name`: the name as written, else the key, else
    /// its declaration.
    fn located(k: &Key, name: &str) -> Option<Span> {
        let found = Cell::new(None);
        k.expr.any(&|x| {
            let at = match x {
                Expr::Located(s, e) if matches!(&**e, Expr::Var(n) if n == name) => Some(*s),
                Expr::Call(_, args) => args.iter().find_map(|a| match a {
                    Arg::Ref(r) if r.index.is_none() && r.name == name => r.span,
                    _ => None,
                }),
                _ => None,
            };
            found.set(at);
            at.is_some()
        });
        found
            .get()
            .or(match k.expr {
                Expr::Located(s, _) => Some(*s),
                _ => None,
            })
            .or(k.at)
    }
    /// A key that reads what was set from a hidden attribute not yet
    /// revealed, refused with `Program::validate`'s words for the attribute
    /// itself, which it refuses at every moment and is left to it.
    fn keys(ks: &[Key], t: &Taint) -> LResult<()> {
        if ks.is_empty() {
            return Ok(());
        }
        let copies = copies(t);
        for k in ks {
            if let Some((n, from)) = reads_unrevealed(k.expr, &copies) {
                let hidden = from
                    .possible
                    .difference(&t.revealed)
                    .next()
                    .map_or("", String::as_str);
                return Err(LinkError::new(format!(
                    "{}: {}, but is read in {}\nhelp: a run whose work reads `{hidden}` reveals \
                     it when the run ends (the end of a decode is the EOS the scheduler sees), \
                     and from there a key may read what was set from it; before, the key is the \
                     scheduler reading what it cannot see",
                    k.owner,
                    why(&n, from, t),
                    k.moment
                ))
                .at(located(k, &n)));
            }
        }
        Ok(())
    }
    /// Where the session waits inside the holds `held`, outermost first:
    /// what reads it wherever it is, and the preempt keys of the pools it
    /// holds. A hold on a pool that preempts goes back to a queue with the
    /// attributes the session has then (`Interp::preempt`): its header is
    /// read again at its admission (`Interp::wanted`, `pending_now`), and
    /// under `requeue tail` its queue's keys select it again.
    fn waits(t: &Taint, reads: &SchedulerReads, held: &[&Stmt]) -> LResult<()> {
        keys(&reads.anywhere, t)?;
        for &hold in held {
            let Stmt::Hold { pools, .. } = hold else {
                continue;
            };
            let names = || pools.iter().map(|(r, _, _)| r.name.as_str());
            for ks in names().filter_map(|p| reads.victim.get(p)) {
                keys(ks, t)?;
            }
            if names().any(|p| reads.victim.contains_key(p)) {
                header(hold, "admission of a preempted hold", t)?;
            }
            if names().any(|p| reads.requeue.contains(p)) {
                for ks in names().filter_map(|p| reads.select.get(p)) {
                    keys(ks, t)?;
                }
            }
        }
        Ok(())
    }
    fn index(r: &Ref) -> Option<&Expr> {
        r.index.as_deref()
    }
    fn walk<'s>(
        stmts: &'s [Stmt],
        t: &mut Taint,
        reads: &SchedulerReads,
        held: &mut Vec<&'s Stmt>,
    ) -> LResult<()> {
        for s in stmts {
            match s {
                Stmt::Set(n, e) => {
                    // a later `set` of a visible value makes it visible again
                    let from = origins(e, t, "the server");
                    t.attrs.remove(n);
                    if !from.possible.is_empty() {
                        t.attrs.insert(n.clone(), from);
                    }
                }
                Stmt::Observe(..) | Stmt::Drop(_) | Stmt::End | Stmt::Turn => {}
                // the release caches what it keeps as an entry of the
                // session's attributes as they are now
                Stmt::Release(_) => keys(&reads.anywhere, t)?,
                Stmt::Run {
                    stage,
                    work,
                    growing,
                    also,
                    latency,
                    ..
                } => {
                    for r in std::iter::once(stage).chain(growing).chain(also) {
                        if let Some(i) = index(r) {
                            check(i, "choice of a stage or pool", t)?;
                        }
                    }
                    // the session waits while it runs
                    waits(t, reads, held)?;
                    // the run's end reveals the hidden attributes its work
                    // and its wait read
                    t.revealed.extend(origins(work, t, "the server").certain);
                    if let Some(l) = latency {
                        t.revealed.extend(origins(l, t, "the server").certain);
                    }
                }
                Stmt::Hold {
                    pools, body, lease, ..
                } => {
                    // the header is read at admission, where `Program::validate`
                    // refuses the hidden attribute itself; what the server set from
                    // it is left here
                    header(s, "admission", t)?;
                    if let Some((_, e)) = lease {
                        check(e, "lease", t)?;
                    }
                    // the session waits in each pool's queue, its keys read
                    // on the attributes as they are before the hold
                    for (r, _, _) in pools {
                        if let Some(ks) = reads.select.get(&r.name) {
                            keys(ks, t)?;
                        }
                    }
                    waits(t, reads, held)?;
                    held.push(s);
                    walk(body, t, reads, held)?;
                    held.pop();
                    // its end releases the pools, as `release P;` does, and
                    // starts the lease, whose time is read there
                    // (`Interp::end_hold`)
                    keys(&reads.anywhere, t)?;
                    if let Some((_, e)) = lease {
                        check(e, "lease", t)?;
                    }
                }
                Stmt::Grow(r, e) | Stmt::Shrink(r, e) | Stmt::Load(r, e) => {
                    if let Some(i) = index(r) {
                        check(i, "choice of a pool", t)?;
                    }
                    check(e, "allocation", t)?;
                    // a growth may wait for room, and a load for its copy;
                    // a shrink frees units, and admits what waits there, as
                    // a release does
                    if matches!(s, Stmt::Shrink(..)) {
                        keys(&reads.anywhere, t)?;
                    } else {
                        waits(t, reads, held)?;
                    }
                }
                // the engine admits a request by its input, as it admits a
                // hold by its header; the output is what the request runs,
                // and its end reveals what it reads, as a run's does
                Stmt::EngineRequest {
                    stage,
                    input,
                    output,
                } => {
                    if let Some(i) = index(stage) {
                        check(i, "choice of an engine", t)?;
                    }
                    // the input is the request's prompt, which the
                    // scheduler knows
                    check(input, "admission", t)?;
                    // the admission's clauses, as a hold's header: the
                    // hidden attribute itself `Program::validate` refuses
                    let derived = copies(t);
                    let engine = reads.engines.get(&stage.name);
                    let read = engine.map_or(&[][..], |e| e.admission.as_slice());
                    for e in read {
                        check(e, "admission", &derived)?;
                    }
                    // and the call's output, when they read `request.output`
                    let reads_output = read
                        .iter()
                        .any(|e| names(e).iter().any(|n| n == "request.output"));
                    if reads_output {
                        check(output, "admission", t)?;
                    }
                    // until its end the request waits in the engine's queue
                    // and runs in its iterations, holding its pools
                    if let Some(engine) = engine {
                        keys(&engine.serve, t)?;
                        for p in &engine.pools {
                            for ks in [reads.select.get(p), reads.victim.get(p)]
                                .into_iter()
                                .flatten()
                            {
                                keys(ks, t)?;
                            }
                        }
                    }
                    waits(t, reads, held)?;
                    t.revealed.extend(origins(output, t, "the server").certain);
                }
                Stmt::Branch(c, a, b) => {
                    check(c, "`if`", t)?;
                    let mut other = t.clone();
                    walk(a, t, reads, held)?;
                    walk(b, &mut other, reads, held)?;
                    join(t, &other);
                }
                // The loop header can receive hidden values after arbitrarily
                // many passes (a <- b <- c <- hidden). Accumulate every incoming
                // origin until the finite set of names/origins stops growing.
                Stmt::While(_, body) | Stmt::Loop(body) => loop {
                    if let Stmt::While(c, _) = s {
                        check(c, "while", t)?;
                    }
                    let before = t.clone();
                    let mut after = before.clone();
                    walk(body, &mut after, reads, held)?;
                    join(t, &after);
                    if *t == before {
                        break;
                    }
                },
                Stmt::Choose { count, key, .. } => {
                    check(count, "choice", t)?;
                    for k in key {
                        check(k, "choice", t)?;
                    }
                }
                // a leg runs on a copy of the attributes: what it sets or
                // reveals stays in it, and it holds none of its parent's
                // pools (`Interp::fork`)
                Stmt::Fork(body) => {
                    let mut leg = t.clone();
                    walk(body, &mut leg, reads, &mut vec![])?;
                }
                Stmt::Declare(..)
                | Stmt::Side(_)
                | Stmt::Route(_)
                | Stmt::Request
                | Stmt::Call { .. }
                | Stmt::Unread(_) => {}
            }
        }
        Ok(())
    }
    fn unrevealed(h: &str) -> Origins {
        Origins {
            possible: BTreeSet::from([h.to_string()]),
            certain: BTreeSet::from([h.to_string()]),
            ..Default::default()
        }
    }
    /// What the workload holds from an unrevealed hidden value along the
    /// session, collected into `at` at each request and into `waiting`
    /// wherever it waits. The server between its `Side` markers is the
    /// request, walked by `walk`.
    struct Workload<'a> {
        turn: &'a [Stmt],
        hidden: &'a [String],
        at: Option<Taint>,
        waiting: Option<Taint>,
    }
    fn gather(into: &mut Option<Taint>, t: &Taint) {
        match into {
            Some(at) => join(at, t),
            None => *into = Some(t.clone()),
        }
    }
    fn workload(stmts: &[Stmt], site: &'static str, t: &mut Taint, w: &mut Workload) {
        let mut in_server = false;
        for s in stmts {
            match s {
                Stmt::Side(crate::ir::Side::Server) => {
                    gather(&mut w.at, t);
                    in_server = true;
                }
                // the request has ended: the workload sees the turn's outputs
                Stmt::Side(crate::ir::Side::Workload) => {
                    t.attrs.clear();
                    in_server = false;
                }
                _ if in_server => {}
                // a hidden attribute set again is unrevealed again
                Stmt::Set(n, _) if w.hidden.contains(n) => {
                    t.attrs.insert(n.clone(), unrevealed(n));
                }
                Stmt::Set(n, e) => {
                    let from = origins(e, t, site);
                    t.attrs.remove(n);
                    if !from.possible.is_empty() {
                        t.attrs.insert(n.clone(), from);
                    }
                }
                Stmt::Turn => {
                    let turn = w.turn;
                    workload(turn, "the workload's turn", t, w);
                }
                Stmt::Branch(_, a, b) => {
                    let mut other = t.clone();
                    workload(a, site, t, w);
                    workload(b, site, &mut other, w);
                    join(t, &other);
                }
                Stmt::While(_, body) | Stmt::Loop(body) => loop {
                    let before = t.clone();
                    let mut after = before.clone();
                    workload(body, site, &mut after, w);
                    join(t, &after);
                    if *t == before {
                        break;
                    }
                },
                // the session waits in the client's stage or pool, where
                // the deployment's eviction and lease keys still read it;
                // and what frees the client's pool admits what waits
                // (`Interp::try_admit_all`), whose admission may evict an
                // entry this session wrote, as the server's `release` does
                Stmt::Run { .. }
                | Stmt::Grow(..)
                | Stmt::Load(..)
                | Stmt::Shrink(..)
                | Stmt::Release(_) => gather(&mut w.waiting, t),
                Stmt::Hold { body, .. } => {
                    gather(&mut w.waiting, t);
                    workload(body, site, t, w);
                    // its end frees its pools
                    gather(&mut w.waiting, t);
                }
                Stmt::Fork(body) => workload(body, site, &mut t.clone(), w),
                // every statement is named, so a new one that holds a block
                // does not compile until this walk says how it enters it
                // (#740); `drop` frees a cached entry, which admits no one
                Stmt::Route(_)
                | Stmt::Declare(..)
                | Stmt::Request
                | Stmt::Observe(..)
                | Stmt::Drop(_)
                | Stmt::Choose { .. }
                | Stmt::End
                | Stmt::Call { .. }
                | Stmt::EngineRequest { .. }
                | Stmt::Unread(_) => {}
            }
        }
    }
    if hidden.is_empty() {
        return Ok(());
    }
    let mut t = Taint {
        attrs: hidden.iter().map(|h| (h.clone(), unrevealed(h))).collect(),
        ..Default::default()
    };
    let mut w = Workload {
        turn,
        hidden,
        at: None,
        waiting: None,
    };
    workload(init, "the workload's init", &mut t, &mut w);
    workload(session, "the workload's session", &mut t, &mut w);
    if let Some(waiting) = &w.waiting {
        keys(&reads.anywhere, waiting)?;
    }
    // the server never sees a hidden attribute, whatever the workload saw
    let mut tainted = w.at.unwrap_or_default();
    for h in hidden {
        tainted.attrs.insert(h.clone(), unrevealed(h));
    }
    walk(server, &mut tainted, reads, &mut vec![])?;
    // after the request the session is the workload's, with what the
    // server set
    keys(&reads.anywhere, &tainted)
}

/// Engine member `i` of `n`'s request: every pool or stage family of `n`
/// members it names without an index is its member `i`.
fn request_member(r: &mut crate::ir::CRequest, i: usize, n: usize, who: &str) -> LResult<()> {
    let at = member_ref;
    let expr = member_expr;
    for (q, u, reserve) in &mut r.pools {
        at(q, i, n, who)?;
        expr(u, i, n, who)?;
        if let Some(x) = reserve {
            expr(x, i, n, who)?;
        }
    }
    for x in r.reuse.iter_mut().chain(&mut r.cache) {
        expr(x, i, n, who)?;
    }
    for (k, m) in r.prefix.iter_mut().flatten() {
        expr(k, i, n, who)?;
        expr(m, i, n, who)?;
    }
    Ok(())
}

/// Member `i` of an `n`-engine family's reference to a family of `n`
/// written without an index.
fn member_ref(c: &mut CRef, i: usize, n: usize, who: &str) -> LResult<()> {
    if c.index.is_none() && c.count > 1 {
        c.base = member(c.base, c.count, i, n, who, "taking")?;
        c.count = 1;
    }
    Ok(())
}

/// `member_ref` for every reference in `e`.
fn member_expr(e: &mut CExpr, i: usize, n: usize, who: &str) -> LResult<()> {
    let expr = member_expr;
    let at = member_ref;
    match e {
        CExpr::Cost(_, x) | CExpr::Unary(_, x) => expr(x, i, n, who),
        CExpr::Binary(_, a, b) => {
            expr(a, i, n, who)?;
            expr(b, i, n, who)
        }
        CExpr::Cond(c, a, b) => {
            expr(c, i, n, who)?;
            expr(a, i, n, who)?;
            expr(b, i, n, who)
        }
        CExpr::Sample(_, xs) => xs.iter_mut().try_for_each(|x| expr(x, i, n, who)),
        CExpr::Call(_, args) => args.iter_mut().try_for_each(|a| match a {
            crate::ir::CArg::Expr(x) => expr(x, i, n, who),
            crate::ir::CArg::Pool(c) | crate::ir::CArg::Stage(c) => at(c, i, n, who),
        }),
        _ => Ok(()),
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
    /// What the bare name `n` means read as a value, if anything: the one
    /// order of the linker's lookups (#538).
    fn resolve(&self, n: &str) -> Option<Meaning> {
        if let Some(&i) = self.attr_index.get(n) {
            Some(Meaning::Attr(i))
        } else if let Some(&v) = self.consts.get(n) {
            Some(Meaning::Const(v))
        } else if let Some(&r) = self.reg_index.get(n) {
            Some(Meaning::Reg(r))
        } else {
            supplied(n).map(Meaning::Supplied)
        }
    }

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

    /// `line:col`.
    fn place(&self, span: Span) -> String {
        format!("{}:{}", span.line, span.col)
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

    /// Whether the pool or stage `name` was declared as a family, `[N]`.
    fn declared_family(&self, what: &str, name: &str) -> bool {
        match what {
            "pool" => self.prog.pools.iter().any(|p| p.name == name && p.array),
            "stage" => self.prog.stages.iter().any(|s| s.name == name && s.array),
            _ => false,
        }
    }

    fn cref(&self, r: &Ref, table: &HashMap<String, (usize, usize)>, what: &str) -> LResult<CRef> {
        let &(base, count) = table
            .get(&r.name)
            .ok_or_else(|| self.unknown(what, &r.name).at(r.span))?;
        let index = match &r.index {
            None if self.member_of.get() == Some(count) => None,
            None => {
                // a family is indexed at every size, so a constant that sizes
                // it does not change how the program is spelled (#220)
                if count != 1 || self.declared_family(what, &r.name) {
                    return Err(LinkError::new(format!(
                        "{what} `{}` is a family of {count}; index it",
                        r.name
                    ))
                    .at(r.span));
                }
                None
            }
            Some(e) => {
                let before = self.terms_written.get();
                let i = self.expr(e)?;
                let after = self.terms_written.get();
                // a constant index (`kv[2]`, `kv[-1]`, `kv[N + 1]`, or one an
                // aggregate wrote out) is checked here, where the program
                // still has its names; one that reads state links as before
                // the fold writes the index's aggregates out again: against
                // the budget they met the first time, and without spending it
                self.terms_written.set(before);
                let folded = match i {
                    // already a number (`blocksize(p)` folds here, not in `eval_const`)
                    CExpr::Num(k) => Some(k),
                    _ => (!e.draws()).then(|| self.eval_const(e).ok()).flatten(),
                };
                self.terms_written.set(after);
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
        self.paired_station(&r.name, "pool")
            .map_err(|e| e.at(r.span))?;
        self.cref(r, &self.pools, "pool")
    }

    fn stage_ref(&self, r: &Ref) -> LResult<CRef> {
        self.paired_station(&r.name, "stage")
            .map_err(|e| e.at(r.span))?;
        self.cref(r, &self.stages, "stage")
    }

    fn cost(&self, args: &[Arg]) -> LResult<CExpr> {
        let Some((value, resources)) = args.split_last() else {
            return Err(LinkError::new(
                "cost takes resources followed by an expression".into(),
            ));
        };
        let mut targets = vec![];
        for arg in resources {
            let Arg::Ref(r) = arg else {
                return Err(LinkError::new(
                    "cost expects resource names before its expression".into(),
                ));
            };
            // An indexed reference (including a def's substituted
            // resource argument) projects its family's type. Check
            // the written index before erasing the annotation.
            if r.index.as_deref().is_some_and(Expr::draws) {
                return Err(LinkError::new(
                    "a cost type annotation cannot draw a member index".into(),
                ));
            }
            let what = if self.pools.contains_key(&r.name) {
                "pool"
            } else {
                "stage"
            };
            self.paired_station(&r.name, what)
                .map_err(|e| e.at(r.span))?;
            targets.push(if let Some(&(base, count)) = self.pools.get(&r.name) {
                if r.index.is_some() {
                    self.pool_ref(r)?;
                }
                crate::ir::CostTarget::Pool { base, count }
            } else if let Some(&(base, count)) = self.stages.get(&r.name) {
                if r.index.is_some() {
                    self.stage_ref(r)?;
                }
                crate::ir::CostTarget::Stage { base, count }
            } else {
                return Err(self.unknown("cost resource", &r.name));
            });
        }
        if targets.is_empty() {
            return Err(LinkError::new("cost requires at least one resource".into()));
        }
        let target = crate::ir::CostTarget::joint(targets);
        let value = match value {
            Arg::Expr(e) => self.expr(e)?,
            Arg::Ref(r) => self.expr(&self.as_number("cost", r)?)?,
        };
        Ok(CExpr::Cost(target, Box::new(value)))
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
            Some(b) => self.eval_const_named(b, &format!("`blocksize({})`", r.name)),
            None => Err(LinkError::new(format!(
                "`blocksize({})`: pool `{}` has no `block`",
                r.name, r.name
            ))
            .at(r.span)),
        }
    }

    /// An engine's `each at most`: its condition may read the iteration's
    /// start, and each cap it chooses is a constant once the `let`s are
    /// known, above 0 (`inf` for none, which the kernel writes 0), or
    /// `max(k, e)` with `k` such a constant and `e` read as the condition is
    /// (vLLM's adaptive threshold). A cap of 0 or below would run as none,
    /// since the kernel reads 0 so; `k` keeps every outcome above it.
    fn per_run_cap(&self, e: &Expr, stage: &str, span: Option<Span>) -> LResult<CExpr> {
        Ok(match e {
            Expr::Located(s, a) => self.per_run_cap(a, stage, Some(*s))?,
            // a condition known once linked chooses its branch here, so the
            // other may be vLLM's 0 for none: `c > 0 ? c : inf`; its
            // indices are still its own, as in `eval_const` (#561)
            Expr::Cond(k, a, b) => match self.eval_const(k) {
                Ok(c) if !c.is_nan() => {
                    let (taken, other) = if c != 0.0 { (a, b) } else { (b, a) };
                    self.own_indices(other)?;
                    self.per_run_cap(taken, stage, span)?
                }
                _ => CExpr::Cond(
                    Box::new(self.expr(k)?),
                    Box::new(self.per_run_cap(a, stage, span)?),
                    Box::new(self.per_run_cap(b, stage, span)?),
                ),
            },
            leaf => {
                // a name it does not know is that error, not this one
                let lowered = self.expr(leaf).map_err(|e| e.at(span))?;
                let v = match self.eval_const(leaf) {
                    Ok(v) => v,
                    Err(_) => return self.floored_cap(leaf, lowered, stage, span),
                };
                if v.is_nan() || v <= 0.0 {
                    return Err(LinkError::new(format!(
                        "engine `{stage}`: `each at most ({v})`: a run would be given nothing, \
                         and the kernel reads 0 as no cap; no cap is written `inf`"
                    ))
                    .at(span));
                }
                CExpr::Num(if v.is_infinite() { 0.0 } else { v })
            }
        })
    }

    /// A cap an `each at most` computes: `max(k, e)`, in either order, with
    /// `k` a constant above 0 once the `let`s are known, so the run gets at
    /// least `k` whatever `e` reads (a NaN `e` included: `max` gives `k`).
    fn floored_cap(
        &self,
        leaf: &Expr,
        lowered: CExpr,
        stage: &str,
        span: Option<Span>,
    ) -> LResult<CExpr> {
        let rule = "an `each at most` is a constant, `inf`, or `max(k, e)` with `k` a \
                    constant above 0, so that no run is given 0 or below, which the kernel \
                    reads as no cap; its condition may read the iteration (write the `?:` \
                    outermost: `c ? 8 : 16`, not `(c ? 4 : 8) * 2`)";
        let unwrap = |e: &Expr| -> Expr {
            let mut e = e;
            while let Expr::Located(_, a) = e {
                e = a;
            }
            e.clone()
        };
        let Expr::Call(f, args) = unwrap(leaf) else {
            return Err(LinkError::new(format!("engine `{stage}`: {rule}")).at(span));
        };
        if f == "min" {
            return Err(LinkError::new(format!(
                "engine `{stage}`: `each at most (min(…))` could give a run 0 or below; a cap \
                 that shrinks writes its floor: `max(1, min(k, e))`; {rule}"
            ))
            .at(span));
        }
        if f != "max" || args.len() != 2 {
            return Err(LinkError::new(format!("engine `{stage}`: {rule}")).at(span));
        }
        let floor = args
            .iter()
            .filter_map(|a| match a {
                Arg::Expr(e) => self.eval_const(e).ok(),
                Arg::Ref(r) if r.index.is_none() => {
                    self.eval_const(&Expr::Var(r.name.clone())).ok()
                }
                Arg::Ref(_) => None,
            })
            .next();
        match floor {
            Some(k) if k > 0.0 && k.is_finite() => Ok(lowered),
            Some(k) if k.is_infinite() && k > 0.0 => Err(LinkError::new(format!(
                "engine `{stage}`: `each at most (max(inf, …))` is no cap whatever it \
                 reads; write `inf`"
            ))
            .at(span)),
            Some(k) => Err(LinkError::new(format!(
                "engine `{stage}`: `each at most (max({k}, …))`: a run could be given \
                 {k}; {rule}"
            ))
            .at(span)),
            None => Err(LinkError::new(format!("engine `{stage}`: {rule}")).at(span)),
        }
    }

    /// A constant that counts (sessions, arrivals): a whole number from `min`
    /// to `max`. A cast would have made -1 a 0, 2.5 a 2 and 1e30 a run that
    /// never starts (#289).
    fn const_count(&self, e: &Expr, what: &str, min: usize, max: usize) -> LResult<usize> {
        let v = self.eval_const_named(e, what)?;
        if !(v.fract() == 0.0 && v >= min as f64 && v <= max as f64) {
            return Err(LinkError::new(format!(
                "{what} is {v}: a count is a whole number from {min} to {max}"
            )));
        }
        Ok(v as usize)
    }

    fn over_expr(&self, agg: Agg, j: &str, n: &Expr, body: &Expr) -> LResult<CExpr> {
        let terms = self.over_terms(agg, j, n, body)?;
        let mut values = terms.iter().map(|e| self.expr(e));
        let first = values.next().expect("positive aggregate count")?;
        values.try_fold(first, |acc, value| {
            let value = value?;
            Ok(match agg {
                Agg::Sum => CExpr::Binary(BinOp::Add, Box::new(acc), Box::new(value)),
                Agg::Min | Agg::Max => CExpr::Call(
                    if agg == Agg::Min { Fun::Min } else { Fun::Max },
                    vec![CArg::Expr(acc), CArg::Expr(value)],
                ),
            })
        })
    }

    /// Resolve an argument a use does not read (`Expr::Unread`) where it
    /// stands, and discard it: a pool or a stage as a reference, any other
    /// name as a value. Never evaluated, it is not checked for its moment.
    fn unread(&self, a: &Arg) -> LResult<()> {
        match a {
            Arg::Expr(e) => self.expr(e).map(drop),
            Arg::Ref(r)
                if self.pools.contains_key(&r.name) || self.stages.contains_key(&r.name) =>
            {
                r.index
                    .as_deref()
                    .map_or(Ok(()), |i| self.expr(i).map(drop))
            }
            Arg::Ref(r) => {
                self.expr(&Expr::Var(r.name.clone()))
                    .map_err(|e| e.at(r.span))?;
                match r.index {
                    Some(_) => Err(self.indexed_value(r).at(r.span)),
                    None => Ok(()),
                }
            }
        }
    }

    /// A name passed where `f` takes a number (`takes_number`), as the value
    /// it names, or why it is none as written (#469): a pool or a stage,
    /// whole or a member, is no number, and a value has no index. A bare
    /// name that is no place, or reads a value though a stage shares its
    /// name (`set svc = 3` beside `stage svc`), is the value: the caller
    /// resolves it, or refuses it as unknown or no constant.
    fn as_number(&self, f: &str, r: &Ref) -> LResult<Expr> {
        let n = &r.name;
        let value = self.expr(&Expr::Var(n.clone()));
        let place = self.pools.contains_key(n) || self.stages.contains_key(n);
        if r.index.is_none() && (value.is_ok() || !place) {
            let var = Expr::Var(n.clone());
            return Ok(match r.span {
                Some(span) => Expr::Located(span, Box::new(var)),
                None => var,
            });
        }
        let written = match r.index {
            Some(_) => format!("{n}[…]"),
            None => n.clone(),
        };
        let why = if self.pools.contains_key(n) {
            LinkError::new(format!(
                "`{f}` reads `{written}` as a number, but a pool is none: read \
                 `queued({written})` or `holders({written})`"
            ))
        } else if self.stages.contains_key(n) {
            LinkError::new(format!(
                "`{f}` reads `{written}` as a number, but a stage is none: read \
                 `budget_left({written})` or `work({written})`"
            ))
        } else {
            match value {
                Err(unknown) => unknown,
                Ok(_) => self.indexed_value(r),
            }
        };
        Err(why.at(r.span))
    }

    /// The refusal for `n[…]` where `n` names a value: it has no members.
    fn indexed_value(&self, r: &Ref) -> LinkError {
        let n = &r.name;
        let what = if self.attr_index.contains_key(n) {
            "an attribute"
        } else {
            "a number"
        };
        LinkError::new(format!("`{n}[…]`: {what} has no index: write `{n}`"))
    }

    fn expr(&self, e: &Expr) -> LResult<CExpr> {
        Ok(match e {
            Expr::Located(span, inner) => self.expr(inner).map_err(|e| e.at(Some(*span)))?,
            Expr::Unread(args, body) => {
                for a in args {
                    self.unread(a)?;
                }
                self.expr(body)?
            }
            Expr::Num(x) => CExpr::Num(*x),
            Expr::Var(n) if n.starts_with("reply.") => {
                let field = &n["reply.".len()..];
                match self.attr_index.get(field) {
                    Some(&i) => CExpr::Attr(i),
                    None => return Err(self.unknown("name", n)),
                }
            }
            Expr::Var(n) => match self.cross(n).map(|()| self.resolve(n))? {
                Some(Meaning::Attr(i)) => CExpr::Attr(i),
                Some(Meaning::Const(v)) => CExpr::Num(v),
                Some(Meaning::Reg(r)) => CExpr::Reg(r),
                Some(Meaning::Supplied(Supplied::Ctx(v))) => CExpr::Ctx(v),
                Some(Meaning::Supplied(Supplied::Inf)) => CExpr::Num(f64::INFINITY),
                None => {
                    return Err(match RENAMED.iter().find(|(old, _)| old == n) {
                        Some((_, new)) => {
                            LinkError::new(format!("the context variable `{n}` is now `{new}`"))
                        }
                        None => self.unknown("name", n),
                    });
                }
            },
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
            Expr::Call(f, args) if f == "cost" => self.cost(args)?,
            Expr::Call(f, args) => {
                let Some(fun) = Fun::from_name(f) else {
                    return Err(unknown_function(f));
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
                        (ArgKind::Expr, Arg::Ref(r)) => {
                            CArg::Expr(self.expr(&self.as_number(f, r)?)?)
                        }
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
            Expr::Over(agg, j, n, body) => self.over_expr(*agg, j, n, body)?,
        })
    }

    /// An engine's `taking`, read for each request; `link` fills in its
    /// records once the program's observations have their places.
    fn request(
        &mut self,
        r: &crate::frontend::ast::RequestSpec,
        engine: &str,
    ) -> LResult<crate::ir::CRequest> {
        let size = self.stages.get(engine).map_or(1, |&(_, c)| c);
        self.member_of.set(Some(size));
        let read = self.request_clauses(r, engine);
        self.member_of.set(None);
        read
    }

    fn request_clauses(
        &mut self,
        r: &crate::frontend::ast::RequestSpec,
        engine: &str,
    ) -> LResult<crate::ir::CRequest> {
        let pools = r
            .pools
            .iter()
            .map(|(q, u, f)| {
                Ok((
                    self.pool_ref(q)?,
                    self.expr(u)?,
                    f.as_ref().map(|f| self.expr(f)).transpose()?,
                ))
            })
            .collect::<LResult<Vec<_>>>()?;
        let prefix = r
            .prefix
            .as_ref()
            .map(|segs| {
                segs.iter()
                    .map(|(k, n)| Ok((self.expr(k)?, self.expr(n)?)))
                    .collect::<LResult<Vec<_>>>()
            })
            .transpose()?;
        Ok(crate::ir::CRequest {
            pools,
            reuse: r.reuse.as_ref().map(|e| self.expr(e)).transpose()?,
            cache: r.cache.as_ref().map(|e| self.expr(e)).transpose()?,
            prefix,
            // a KV relation's source: set with the relations
            send: None,
            // the attributes now, so that the caller's `E.first` reads one
            records: crate::ir::RequestRecords {
                reused: 0,
                prefill: 0,
                admitted: self.attr(&format!("{engine}.{}", REQUEST_ATTRS[0])),
                first: self.attr(&format!("{engine}.{}", REQUEST_ATTRS[1])),
                last_reused: self.attr(&format!("{engine}.{}", REQUEST_ATTRS[2])),
                last_prefill: self.attr(&format!("{engine}.{}", REQUEST_ATTRS[3])),
            },
        })
    }

    /// An engine's connector, for member `member` of an engine family (`None`
    /// for one engine): its amounts, read for the request, and the kernel
    /// blocks they mean, which read them as `E.loaded` and `E.saved`
    /// (`ir::CConnector`). A load computes into the request's hold on
    /// `memory`, the engine's memory its `taking` takes: an L2's copies its
    /// tokens at the L2's stage, a KV relation's reads them over the two
    /// NICs from the lease the request's entry took `from` the source (the
    /// member `E.from`), waiting first for the source's leg in a push, and
    /// ends the lease. A save is the hold on the L2's pool that copies what
    /// its entry lacks and caches the rest.
    fn connector(
        &mut self,
        c: &ConnectorSpec,
        engine: &str,
        memory: Option<&Ref>,
        member: Option<usize>,
    ) -> LResult<crate::ir::CConnector> {
        let [loaded, saved, load_done] = CONNECTOR_ATTRS.map(|a| format!("{engine}.{a}"));
        let slots = [&loaded, &saved, &load_done].map(|n| self.attr(n));
        let var = |n: &str| Expr::Var(n.into());
        let one = std::slice::from_ref;
        // the engine's own pools and stages, as its `taking` names them: a
        // family's member is written in (`member_block`)
        let own = |name: &str| Ref {
            span: None,
            name: name.into(),
            index: None,
        };
        let size = self.stages.get(engine).map_or(1, |&(_, c)| c);
        let side = self.side;
        self.side = crate::ir::Side::Server;
        let load = match &c.load {
            None => None,
            Some(n) => {
                let kv = memory.expect("refused by `connector_untaken`");
                let kv = own(&kv.name);
                let mut body = match &c.kind {
                    ConnectorKind::L2 {
                        stage, per_token, ..
                    } => vec![Stmt::Run {
                        stage: stage.clone(),
                        mode: RunMode::Plain,
                        work: Expr::cost(
                            one(stage),
                            Expr::Binary(
                                BinOp::Mul,
                                Box::new(var(&loaded)),
                                Box::new(per_token.clone()),
                            ),
                        ),
                        growing: None,
                        also: vec![],
                        share: None,
                        latency: None,
                        poster: None,
                    }],
                    ConnectorKind::Relation(None) => {
                        unreachable!("the parser refuses a connector no relation reads by")
                    }
                    ConnectorKind::Relation(Some(rel)) => {
                        // a source family is indexed at every size, a family
                        // of one too (`P[1]`), by the member the entry took
                        // its KV from
                        let src = |what: &str, name: String| Ref {
                            span: None,
                            index: self
                                .declared_family(what, &name)
                                .then(|| Box::new(var(&format!("{engine}.{}", RELATION_FROM)))),
                            name,
                        };
                        // the reader waits for the source's send to be
                        // registered (a push's leg), which its load ends
                        let mut out = vec![];
                        // the side that posts the copy waits before it: the
                        // copy's run's `latency`
                        let nic = own(&format!("{}.nic", rel.reader));
                        out.push(Stmt::Run {
                            stage: src("stage", format!("{}.nic", rel.source)),
                            mode: RunMode::Plain,
                            work: Expr::cost(
                                &[
                                    Ref {
                                        span: None,
                                        name: format!("{}.nic", rel.source),
                                        index: None,
                                    },
                                    nic.clone(),
                                ],
                                var(&loaded),
                            ),
                            growing: None,
                            also: vec![nic],
                            share: Some(rel.share),
                            latency: rel.latency.clone(),
                            // the source's NIC is the run's stage, the
                            // reader's its `also`
                            poster: Some(if rel.push { 0 } else { 1 }),
                        });
                        out.push(Stmt::Release(src("pool", rel.leased.clone())));
                        out
                    }
                };
                let at = body.len() - usize::from(matches!(c.kind, ConnectorKind::Relation(_)));
                body.insert(
                    at,
                    Stmt::Load(kv.clone(), Expr::cost(one(&kv), var(&loaded))),
                );
                self.member_of.set(member.map(|_| size));
                let first = self.blocks.len();
                let compiled = self.expr(n).and_then(|e| Ok((e, self.block(&body)?)));
                self.member_of.set(None);
                let (mut amount, b) = compiled?;
                if let Some(i) = member {
                    member_expr(&mut amount, i, size, engine)?;
                    self.member_block(b, i, size, engine)?;
                }
                Some(transfer(amount, self.take_blocks(first)))
            }
        };
        let save = match (&c.save, &c.kind) {
            (None, _) => None,
            (Some(_), ConnectorKind::Relation(_)) => unreachable!("the parser refuses it"),
            (
                Some(n),
                ConnectorKind::L2 {
                    stage,
                    pool,
                    per_token,
                },
            ) => {
                // the part of what the L2 is to hold that its entry lacks
                let new = Expr::Call(
                    "max".into(),
                    vec![
                        Arg::Expr(Expr::Num(0.0)),
                        Arg::Expr(Expr::Binary(
                            BinOp::Sub,
                            Box::new(var(&saved)),
                            Box::new(var("cached")),
                        )),
                    ],
                );
                let kv = pool;
                let held = Expr::Call("cachedin".into(), vec![Arg::Ref(kv.clone())]);
                let body = vec![Stmt::Hold {
                    pools: vec![(kv.clone(), Expr::cost(one(kv), held), None)],
                    reuse: None,
                    body: vec![
                        Stmt::Grow(kv.clone(), Expr::cost(one(kv), new.clone())),
                        Stmt::Run {
                            stage: stage.clone(),
                            mode: RunMode::Plain,
                            work: Expr::cost(
                                one(stage),
                                Expr::Binary(
                                    BinOp::Mul,
                                    Box::new(new.clone()),
                                    Box::new(per_token.clone()),
                                ),
                            ),
                            growing: None,
                            also: vec![],
                            share: None,
                            latency: None,
                            poster: None,
                        },
                        Stmt::Load(kv.clone(), Expr::cost(one(kv), new)),
                    ],
                    cache: Some(Expr::cost(one(kv), var(&saved))),
                    prefix: None,
                    lease: None,
                }];
                let amount = self.expr(n)?;
                let first = self.blocks.len();
                self.block(&body)?;
                Some(transfer(amount, self.take_blocks(first)))
            }
        };
        self.side = side;
        Ok(crate::ir::CConnector {
            load,
            save,
            loaded: slots[0],
            saved: slots[1],
            load_done: slots[2],
            from: matches!(c.kind, ConnectorKind::Relation(_))
                .then(|| self.attr(&format!("{engine}.{RELATION_FROM}"))),
            record: self.observe_slot(&loaded),
        })
    }

    /// The blocks from `first` on, taken out of the program: a connector's,
    /// which `transfer` reads as its transfer's fixed shape.
    fn take_blocks(&mut self, first: usize) -> Vec<Vec<CStmt>> {
        self.spans.truncate(first);
        self.sides.truncate(first);
        self.blocks.split_off(first)
    }

    /// `member_ref` for the references of a connector's block.
    fn member_block(&mut self, b: BlockId, i: usize, n: usize, who: &str) -> LResult<()> {
        let mut stmts = std::mem::take(&mut self.blocks[b]);
        for s in &mut stmts {
            match s {
                // a run's stages are its member by index, as its work's
                // cost is the family's
                CStmt::Run {
                    stage,
                    growing,
                    also,
                    ..
                } => {
                    for r in std::iter::once(stage).chain(also.iter_mut()) {
                        if r.index.is_none() && r.count == n && n > 1 {
                            r.index = Some(Box::new(CExpr::Num(i as f64)));
                        }
                    }
                    if let Some(g) = growing {
                        member_ref(g, i, n, who)?;
                    }
                }
                // a pool its member, and the amount's cost the member's
                CStmt::Load(r, e) | CStmt::Grow(r, e) => {
                    let family = r.index.is_none() && r.count == n && n > 1;
                    member_ref(r, i, n, who)?;
                    member_expr(e, i, n, who)?;
                    if family && let CExpr::Cost(crate::ir::CostTarget::Pool { base, count }, _) = e
                    {
                        *base += i;
                        *count = 1;
                    }
                }
                CStmt::Release(r) => member_ref(r, i, n, who)?,
                _ => {}
            }
        }
        self.blocks[b] = stmts;
        Ok(())
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
        self.spans.push(vec![]);
        self.sides.push(vec![]);
        let initial_side = self.side;
        let reader = self.reader.get();
        let mut side_stack = vec![];
        let mut routes: Vec<(String, usize)> = vec![];
        let mut out = vec![];
        for s in stmts {
            self.reader.set(Some(self.side));
            if let Stmt::Declare(name, kind) = s {
                self.declarations.push((self.attr_index[name], *kind));
                continue;
            }
            if let Stmt::Unread(args) = s {
                for a in args {
                    self.unread(a)?;
                }
                continue;
            }
            // a gateway's route: from the statement after its opening to the
            // one before its close, in this block (`expand` puts both marks
            // in one list)
            if let Stmt::Route(mark) = s {
                let here = self.sides[id].len();
                match mark {
                    RouteMark::Open(g) => routes.push((g.clone(), here)),
                    RouteMark::Close => {
                        let (gateway, from) = routes.pop().expect("`expand` closes what it opens");
                        if from < here {
                            self.routes.push(crate::ir::Route {
                                gateway,
                                block: id,
                                from,
                                to: here,
                            });
                        }
                    }
                }
                continue;
            }
            if let Stmt::Side(side) = s {
                match side {
                    crate::ir::Side::Server => {
                        side_stack.push(self.side);
                        self.side = *side;
                    }
                    crate::ir::Side::Workload => {
                        self.side = side_stack.pop().unwrap_or(initial_side);
                    }
                }
                continue;
            }
            self.spans[id].push(StmtSpan {
                at: stmt_span(s),
                names: stmt_names(s),
            });
            self.sides[id].push(self.side);
            let cs = match s {
                Stmt::Set(n, e) => CStmt::Set(self.attr_index[n], self.expr(e)?),
                Stmt::Observe(n, e) => {
                    let e = self.expr(e)?;
                    CStmt::Observe(self.observe_slot(n), e)
                }
                Stmt::Turn => CStmt::Turn,
                Stmt::End => CStmt::End,
                Stmt::Declare(..) | Stmt::Side(_) | Stmt::Route(_) | Stmt::Unread(_) => {
                    unreachable!()
                }
                Stmt::Request => {
                    return Err(LinkError::new(
                        "`request` survived parsing: the parser splices the server in its place"
                            .into(),
                    ));
                }
                Stmt::Call { .. } => {
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
                    prefix,
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
                    let prefix = prefix
                        .as_ref()
                        .map(|segs| {
                            segs.iter()
                                .map(|(k, n)| Ok((self.expr(k)?, self.expr(n)?)))
                                .collect::<LResult<Vec<_>>>()
                        })
                        .transpose()?;
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
                        prefix,
                        lease,
                    }
                }
                Stmt::Grow(r, e) => CStmt::Grow(self.pool_ref(r)?, self.expr(e)?),
                Stmt::Shrink(r, e) => CStmt::Shrink(self.pool_ref(r)?, self.expr(e)?),
                Stmt::Drop(r) => CStmt::Drop(self.pool_ref(r)?),
                Stmt::Release(r) => CStmt::Release(self.pool_ref(r)?),
                Stmt::Load(r, e) => CStmt::Load(self.pool_ref(r)?, self.expr(e)?),
                Stmt::EngineRequest {
                    stage,
                    input,
                    output,
                } => CStmt::Request {
                    stage: self.stage_ref(stage)?,
                    input: self.expr(input)?,
                    output: self.expr(output)?,
                },
                Stmt::Run {
                    stage,
                    mode,
                    work,
                    growing,
                    also,
                    share,
                    latency,
                    poster,
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
                        share: *share,
                        latency: latency.as_ref().map(|l| self.expr(l)).transpose()?,
                        poster: *poster,
                    }
                }
                Stmt::Branch(p, a, b) => {
                    let p = self.expr(p)?;
                    let a = self.block(a)?;
                    let b = self.block(b)?;
                    CStmt::Branch(p, a, b)
                }
                Stmt::Loop(b) => CStmt::Loop(self.block(b)?),
                Stmt::While(c, b) => CStmt::While(self.expr(c)?, self.block(b)?),
                Stmt::Fork(b) => CStmt::Fork(self.block(b)?),
                Stmt::Choose { var, count, key } => CStmt::Choose {
                    var: self.attr_index[var],
                    count: self.expr(count)?,
                    key: key.iter().map(|k| self.expr(k)).collect::<LResult<_>>()?,
                },
            };
            out.push(cs);
        }
        self.blocks[id] = out;
        self.side = initial_side;
        self.reader.set(reader);
        Ok(id)
    }

    /// For a pair of files: the workload runs on and holds its own stages
    /// and pools (and the stage `tool` it needs none for: its `tool Z;`
    /// runs on whichever file declares it), the deployment none of them
    /// (`docs/design/paired-workload.md`). `what` is `stage` or `pool`: a
    /// pool named `tool` is no stage, and the deployment's is its own.
    fn paired_station(&self, name: &str, what: &str) -> LResult<()> {
        let (Some(wl), Some(side)) = (&self.prog.paired, self.reader.get()) else {
            return Ok(());
        };
        let ours = wl.iter().any(|w| w == name);
        match side {
            crate::ir::Side::Workload if !ours && !(what == "stage" && name == "tool") => {
                Err(LinkError::new(format!(
                    "{what} `{name}` is the deployment file's: the workload runs on and holds \
                     its own stages and pools, and sends the server what it runs"
                )))
            }
            crate::ir::Side::Server if ours => Err(LinkError::new(format!(
                "{what} `{name}` is the workload file's: the deployment runs on and holds its \
                 own stages and pools"
            ))),
            _ => Ok(()),
        }
    }

    /// Refuse a read across the server's signature: the server (and the
    /// scheduler) of what the workload keeps, the workload of what the
    /// server keeps (`docs/design/request-interface.md`).
    fn cross(&self, n: &str) -> LResult<()> {
        let (Some(i), Some(side)) = (&self.prog.interface, self.reader.get()) else {
            return Ok(());
        };
        let names = |v: &[String]| {
            if v.is_empty() {
                "nothing".to_string()
            } else {
                v.iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        };
        match side {
            crate::ir::Side::Server if i.client.iter().any(|c| c == n) => {
                // a trace's attributes a turn draws, the others the workload sets
                let what = if BUILTIN_ATTRS.contains(&n) {
                    "which a turn draws"
                } else {
                    "which the workload sets"
                };
                Err(LinkError::new(format!(
                    "the server reads `{n}`, {what}; a request carries {}\n\
                     help: add `{n}` to `server (…)` and bind it in the workload's `send (…)`",
                    names(&i.params)
                )))
            }
            crate::ir::Side::Workload
                if self.attr_index.contains_key(n) && !i.workload.iter().any(|c| c == n) =>
            {
                if i.params.iter().any(|p| p == n) {
                    return Err(LinkError::new(format!(
                        "the workload reads `{n}`, the server's parameter: `send` binds it, \
                         and the workload reads its own attributes"
                    )));
                }
                let help = if i.reply.iter().any(|r| r == n) {
                    format!("read the reply: `reply.{n}`")
                } else if n.contains('.') {
                    format!(
                        "set it under a name in the server (`set x = {n};`), add `x` to \
                         `reply (…)` and read `reply.x`"
                    )
                } else {
                    format!("add `{n}` to the server's `reply (…)` and read `reply.{n}`")
                };
                Err(LinkError::new(format!(
                    "the workload reads `{n}`, which the server sets; the reply carries {}\n\
                     help: {help}",
                    names(&i.reply)
                )))
            }
            _ => Ok(()),
        }
    }
}

/// What a constant expression may name. The one constant evaluator,
/// `eval_const`, and the aggregates' `over_terms` read it: the linker's
/// knows the whole program, the parser's (`Lets`) only the `let`s folded so
/// far, with which it sizes an array the linker sees as a number. The
/// linker asks the indices of what the parser folded through
/// `Program::folded` (#561).
pub(crate) trait Constants {
    /// The value of the constant `name`, or why there is none.
    fn constant(&self, name: &str) -> LResult<f64>;
    /// A reference `f` takes as a number (`min(N, 2)`), as the name it reads.
    fn ref_as_number(&self, f: &str, r: &Ref) -> LResult<Expr>;
    /// Resolve an argument a use does not read (`Expr::Unread`).
    fn resolve_unread(&self, a: &Arg) -> LResult<()>;
    /// What else an aggregate's index `j` names, if anything.
    fn index_clash(&self, j: &str) -> Option<&'static str>;
    /// Whether `j`, the index of an aggregate `agg`, is a name of its own:
    /// one that also named something else would leave the body saying two
    /// things.
    fn own_index(&self, agg: Agg, j: &str) -> LResult<()> {
        match self.index_clash(j) {
            Some(c) => Err(LinkError::new(format!(
                "`{} {j} in`: `{j}` is also {c}\nhelp: give the index a name of its own",
                agg.name()
            ))),
            None => Ok(()),
        }
    }
    /// [`Self::own_index`] of every aggregate in `e`, refused at the
    /// aggregate, where `e` is not evaluated: a constant the parser folded
    /// (an array size, a `ttl`, whose value is the parser's) or a branch a
    /// constant condition does not take. Either is the program's, and its
    /// indices are asked as an expression's are (#561), in an argument a
    /// call does not read as well (`Expr::any_written`). The parser
    /// wraps every aggregate in its place (`Expr::Located`).
    fn own_indices(&self, e: &Expr) -> LResult<()> {
        let refused = Cell::new(None);
        e.any_written(&|x| {
            let Expr::Located(span, inner) = x else {
                return false;
            };
            let Expr::Over(agg, j, ..) = &**inner else {
                return false;
            };
            let own = self.own_index(*agg, j);
            own.map_err(|e| refused.set(Some(e.at(Some(*span)))))
                .is_err()
        });
        refused.into_inner().map_or(Ok(()), Err)
    }
    /// The terms aggregates have written out so far, against `MAX_OVER`.
    fn terms_written(&self) -> &Cell<usize>;

    /// The value of a constant expression: numbers, `inf` and constants,
    /// combined by the operators, the arithmetic functions (`Fun::arithmetic`), a
    /// condition and an aggregate written out. An attribute, a draw or
    /// `blocksize` is no constant; an unread argument is resolved, as in
    /// any expression, and never evaluated, so it need not be one
    /// (`one(kv)`).
    fn eval_const(&self, e: &Expr) -> LResult<f64> {
        Ok(match e {
            Expr::Located(span, inner) => self.eval_const(inner).map_err(|e| e.at(Some(*span)))?,
            Expr::Num(x) => *x,
            Expr::Var(n) if supplied(n) == Some(Supplied::Inf) => f64::INFINITY,
            Expr::Var(n) => self.constant(n)?,
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
                let (taken, other) = if self.eval_const(c)? != 0.0 {
                    (a, b)
                } else {
                    (b, a)
                };
                // the branch not taken is not evaluated, but it is the
                // program's: its indices are its own (#561)
                self.own_indices(other)?;
                self.eval_const(taken)?
            }
            Expr::Call(f, _) if f == "blocksize" => {
                return Err(LinkError::new(
                    "`blocksize` is not a constant: in a constant position (a `let`, `cap`, \
                     `block`, an array size, `horizon`, a rate) name the value with a `let` \
                     both use"
                        .into(),
                ));
            }
            Expr::Unread(args, body) => {
                for a in args {
                    self.resolve_unread(a)?;
                }
                self.eval_const(body)?
            }
            Expr::Call(f, _) if !crate::frontend::parser::is_function(f) => {
                return Err(unknown_function(f));
            }
            Expr::Call(f, args) => {
                let not_constant = || {
                    LinkError::new(format!(
                        "`{f}` with {} argument(s) is not a constant function",
                        args.len()
                    ))
                };
                let xs: Vec<f64> = args
                    .iter()
                    .enumerate()
                    .map(|(k, a)| match a {
                        Arg::Expr(e) => self.eval_const(e),
                        Arg::Ref(r) if takes_number(f, k, args.len()) => {
                            self.eval_const(&self.ref_as_number(f, r)?)
                        }
                        Arg::Ref(_) => Err(not_constant()),
                    })
                    .collect::<LResult<_>>()?;
                Fun::from_name(f)
                    .and_then(|f| f.arithmetic(&xs))
                    .ok_or_else(not_constant)?
            }
            Expr::Sample(..) => return Err(LinkError::new("a constant cannot sample".into())),
            Expr::Over(agg, j, n, body) => {
                let terms = self.over_terms(*agg, j, n, body)?;
                let mut values = terms.iter().map(|e| self.eval_const(e));
                let first = values.next().expect("positive aggregate count")?;
                values.try_fold(first, |acc, value| {
                    let value = value?;
                    Ok(match agg {
                        Agg::Sum => acc + value,
                        Agg::Min => acc.min(value),
                        Agg::Max => acc.max(value),
                    })
                })?
            }
        })
    }

    /// `eval_const` for the constant `what` (`pool \`kv\`: cap`, `the horizon`,
    /// ...), which names it when it refuses a NaN: a constant is a number or
    /// an infinity. No run means anything with a NaN constant, and the IR
    /// has no spelling for one.
    fn eval_const_named(&self, e: &Expr, what: &str) -> LResult<f64> {
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

    /// `max j in n (e)` written out as separate terms, with `j` = 0, 1, …,
    /// n-1. Callers fold left after linking/evaluating each term: constructing
    /// a deep source tree first makes linking consume one stack frame per term.
    /// `n` is a constant, and `j` a name of its
    /// own: a constant, an attribute, a pool, a stage or a context variable
    /// of the same name would leave the body saying two things.
    fn over_terms(&self, agg: Agg, j: &str, n: &Expr, body: &Expr) -> LResult<Vec<Expr>> {
        self.own_index(agg, j)?;
        let what = format!("`{} {j} in`", agg.name());
        let count = self.eval_const_named(n, &format!("{what}'s count"))?;
        if !(count >= 1.0 && count.fract() == 0.0 && count.is_finite()) {
            return Err(LinkError::new(format!(
                "{what}'s count must be a positive integer, found {count}"
            )));
        }
        // the terms are written out, nested aggregates' included: the
        // program's total is bounded, so no count makes the linker run away
        let total = self.terms_written().get() as f64 + count;
        if total > MAX_OVER as f64 {
            return Err(LinkError::new(format!(
                "{what}: the program's aggregates write out {total} terms, at most {MAX_OVER}"
            )));
        }
        self.terms_written().set(total as usize);
        let term = |k: usize| {
            let mut e = body.clone();
            bind_index(&mut e, j, k as f64);
            e
        };
        Ok((0..count as usize).map(term).collect())
    }
}

/// The linker's constants: the program's `let`s, and every name it declares
/// for an aggregate's index to clash with.
impl Constants for Linker<'_> {
    fn constant(&self, name: &str) -> LResult<f64> {
        self.consts
            .get(name)
            .copied()
            .ok_or_else(|| self.unknown("constant", name))
    }

    fn ref_as_number(&self, f: &str, r: &Ref) -> LResult<Expr> {
        self.as_number(f, r)
    }

    fn resolve_unread(&self, a: &Arg) -> LResult<()> {
        self.unread(a)
    }

    /// What else an aggregate's index names, read as `resolve` reads it: an
    /// index the expression would read as that instead is refused. A `let`
    /// and a register are asked of the program, since the constants are
    /// folded, and the registers taken, one by one: whatever the order the
    /// names are declared in, the index may not take one.
    fn index_clash(&self, j: &str) -> Option<&'static str> {
        if self.prog.lets.iter().any(|(n, _)| n == j) {
            return Some("a `let` constant");
        }
        let a_register = self.prog.stages.iter().any(
            |s| matches!(&s.kind, StageKind::Step(sp) if sp.state.iter().any(|(n, _)| n == j)),
        );
        if a_register {
            return Some("an engine's register");
        }
        match self.resolve(j) {
            Some(Meaning::Attr(_)) => Some("a session attribute"),
            // a folded `let` or a register taken: both answered above
            Some(Meaning::Const(_)) => Some("a `let` constant"),
            Some(Meaning::Reg(_)) => Some("an engine's register"),
            Some(Meaning::Supplied(_)) => Some("a name the language supplies"),
            None if self.pools.contains_key(j) => Some("a pool"),
            None if self.stages.contains_key(j) => Some("a stage"),
            None => None,
        }
    }

    fn terms_written(&self) -> &Cell<usize> {
        &self.terms_written
    }
}

/// The functions taken out of the language, with what a program writes
/// instead. A call is told; the name is free for anything else.
const RETIRED_FUNCTIONS: [(&str, &str); 1] = [("pow", "`pow(x, y)` is now `x ^ y`")];

/// A call of a name that is no function, with what to write instead when
/// the language took the function out.
fn unknown_function(f: &str) -> LinkError {
    match RETIRED_FUNCTIONS.iter().find(|(w, _)| *w == f) {
        Some((_, now)) => LinkError::new(format!("unknown function `{f}`: {now}")),
        None => LinkError::new(format!("unknown function `{f}`")),
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

/// Replace the index `j` of an `Expr::Over` by the number `k`, in the
/// body and in its references' indices; an inner `Over` of the same name
/// keeps its own.
pub(crate) fn bind_index(e: &mut Expr, j: &str, k: f64) {
    e.substitute(&[(j.to_string(), Expr::Num(k))]);
}
