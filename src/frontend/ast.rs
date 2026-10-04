//! Abstract syntax of a serQ program.
//!
//! A program is a *deployment* (pools and stages), a *workload* (how
//! sessions arrive and how a session's attributes evolve from turn to
//! turn) and a *session* (the statements every session executes). A
//! program written as `workload { … session { … request; … } }` and
//! `server { … }` arrives here with the server spliced into the session:
//! the split is the parser's. See `docs/language.md` for the semantics.

pub use crate::frontend::diagnostic::Span;

pub use crate::ir::{BinOp, Preempt, RunMode, UnOp};

/// Expressions are evaluated to `f64`. Booleans are 0 / 1.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// Original identifier location, retained through header substitution.
    Located(Span, Box<Expr>),
    Num(f64),
    /// A session attribute or a `let` constant, resolved at link time.
    Var(String),
    /// `~exp(1.5)`: a fresh draw from a distribution.
    Sample(String, Vec<Expr>),
    /// `min(a, b)`, `price(stage, s, ds)`, `work(prefill[j])`, ...
    Call(String, Vec<Arg>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// `c ? a : b`
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `max j in n (e)`, `min …`, `sum …`: `e` over `j = 0 … n-1`. Sugar:
    /// the linker writes it out with `j` a number, as binary `max`, `min`
    /// or `+`, so `n` is a constant.
    Over(Agg, String, Box<Expr>, Box<Expr>),
}

/// What `Expr::Over` folds its terms with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agg {
    Max,
    Min,
    Sum,
}

impl Agg {
    /// The aggregate a word names when `j in n (e)` follows it: `max` and
    /// `min` are functions otherwise, `sum` is a keyword.
    pub fn from_name(w: &str) -> Option<Agg> {
        [Agg::Max, Agg::Min, Agg::Sum]
            .into_iter()
            .find(|a| a.name() == w)
    }

    pub fn name(self) -> &'static str {
        match self {
            Agg::Max => "max",
            Agg::Min => "min",
            Agg::Sum => "sum",
        }
    }
}

/// A call argument: an expression or a reference to a pool or stage
/// (`kv`, `prefill[j]`).
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Expr(Expr),
    Ref(Ref),
}

/// A pool or stage reference, possibly indexed into an array.
#[derive(Clone, Debug, PartialEq)]
pub struct Ref {
    pub span: Option<Span>,
    pub name: String,
    pub index: Option<Box<Expr>>,
}

impl Ref {
    /// Ownership checks compare the written target, not where it was written.
    /// In particular, q[i] and q[j] remain different targets.
    pub(crate) fn same_target(&self, other: &Self) -> bool {
        self.name == other.name
            && match (&self.index, &other.index) {
                (None, None) => true,
                (Some(a), Some(b)) => a.same_syntax(b),
                _ => false,
            }
    }
}

impl Expr {
    /// Whether `f` holds of this expression or of one in it: the operand of
    /// a `Located`, a reference's index and an aggregate's count and body
    /// included (#280).
    pub fn any(&self, f: &impl Fn(&Expr) -> bool) -> bool {
        f(self)
            || match self {
                Expr::Num(_) | Expr::Var(_) => false,
                Expr::Located(_, x) | Expr::Unary(_, x) => x.any(f),
                Expr::Sample(_, xs) => xs.iter().any(|x| x.any(f)),
                Expr::Call(_, args) => args.iter().any(|a| match a {
                    Arg::Expr(x) => x.any(f),
                    Arg::Ref(r) => r.index.as_ref().is_some_and(|i| i.any(f)),
                }),
                Expr::Binary(_, a, b) | Expr::Over(_, _, a, b) => a.any(f) || b.any(f),
                Expr::Cond(c, a, b) => c.any(f) || a.any(f) || b.any(f),
            }
    }

    /// Replace every `Var(name)` of `binds` by its expression: a header
    /// binding (`at admission (…)`), or an aggregate's index (`max j in n`)
    /// by its number. A bare argument naming one is the binding, not a
    /// reference; an inner aggregate's index is its own.
    pub fn substitute(&mut self, binds: &[(String, Expr)]) {
        match self {
            Expr::Located(_, inner) => inner.substitute(binds),
            Expr::Var(n) => {
                if let Some((_, v)) = binds.iter().find(|(name, _)| name == n) {
                    *self = v.clone();
                }
            }
            Expr::Num(_) => {}
            Expr::Sample(_, args) => args.iter_mut().for_each(|a| a.substitute(binds)),
            Expr::Call(_, args) => args.iter_mut().for_each(|a| match a {
                Arg::Expr(x) => x.substitute(binds),
                Arg::Ref(r) => {
                    // a bare identifier argument is parsed as a reference (it
                    // may name a pool or a stage); when it names a binding it
                    // is the binding, else `min(known, …)` would read the
                    // attribute `known` and not the header's `known = …`
                    if r.index.is_none()
                        && let Some((_, v)) = binds.iter().find(|(name, _)| *name == r.name)
                    {
                        *a = Arg::Expr(v.clone());
                    } else if let Some(i) = &mut r.index {
                        i.substitute(binds);
                    }
                }
            }),
            Expr::Unary(_, a) => a.substitute(binds),
            Expr::Binary(_, a, b) => {
                a.substitute(binds);
                b.substitute(binds);
            }
            Expr::Cond(c, a, b) => {
                c.substitute(binds);
                a.substitute(binds);
                b.substitute(binds);
            }
            Expr::Over(_, j, n, body) => {
                n.substitute(binds);
                let inner: Vec<_> = binds
                    .iter()
                    .filter(|(name, _)| name != j)
                    .cloned()
                    .collect();
                body.substitute(&inner);
            }
        }
    }

    /// Whether the expression draws (`~`) anywhere.
    pub fn draws(&self) -> bool {
        self.any(&|x| matches!(x, Expr::Sample(..)))
    }

    fn same_syntax(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Located(_, a), b) => a.same_syntax(b),
            (a, Self::Located(_, b)) => a.same_syntax(b),
            (Self::Unary(aop, a), Self::Unary(bop, b)) => aop == bop && a.same_syntax(b),
            (Self::Binary(aop, a, b), Self::Binary(bop, c, d)) => {
                aop == bop && a.same_syntax(c) && b.same_syntax(d)
            }
            (Self::Cond(a, b, c), Self::Cond(d, e, f)) => {
                a.same_syntax(d) && b.same_syntax(e) && c.same_syntax(f)
            }
            (Self::Over(f, j, n, a), Self::Over(g, k, m, b)) => {
                f == g && j == k && n.same_syntax(m) && a.same_syntax(b)
            }
            (Self::Sample(a, xs), Self::Sample(b, ys)) => {
                a == b && xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| x.same_syntax(y))
            }
            (Self::Call(a, xs), Self::Call(b, ys)) => {
                a == b
                    && xs.len() == ys.len()
                    && xs.iter().zip(ys).all(|(x, y)| match (x, y) {
                        (Arg::Expr(a), Arg::Expr(b)) => a.same_syntax(b),
                        (Arg::Ref(a), Arg::Ref(b)) => a.same_target(b),
                        _ => false,
                    })
            }
            _ => self == other,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EvictOrder {
    /// Least recently released first.
    Lru,
    /// Ascending lexicographic key, evaluated per cached entry.
    By(Vec<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum QueueOrder {
    Fifo,
    By(Vec<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spill {
    pub to: Ref,
    pub via: Ref,
    pub work: Expr,
    pub when: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PoolDecl {
    pub span: Option<Span>,
    pub name: String,
    pub count: usize,
    /// Declared as an array (`pool kv[N]`): its members carry their index,
    /// a one-member array's too.
    pub array: bool,
    pub cap: Expr,
    pub block: Option<Expr>,
    pub evict: EvictOrder,
    pub preempt: Preempt,
    pub queue: QueueOrder,
    pub spill: Option<Spill>,
    /// `admit via STAGE`: the queue is served by the stage's scheduler, at
    /// the start of its iterations, while the iteration has budget left.
    pub admit_via: Option<Ref>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StageKind {
    /// `c` servers, one job each at rate 1, FIFO.
    Fifo(Expr),
    /// Processor sharing with capacity `phi(present)`, `present` the jobs present.
    Ps(Expr),
    /// Infinite server: every job at rate 1.
    Delay,
    /// Iterating engine (continuous batching with chunked prefill).
    Step(Box<StepSpec>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct StepSpec {
    /// Tokens per iteration (`max_num_batched_tokens`).
    pub budget: Expr,
    /// Seconds per iteration, in `tokens`, `decoders`, `prefilled`, `residents`, `kv_decode`.
    pub cost: Expr,
    /// Cap on one request's prefill chunk (`long_prefill_token_threshold`,
    /// 0 = none).
    pub chunk: Expr,
    /// The order the iteration serves its residents in (`serve …;`).
    pub serve: Serve,
    /// `serve only (expr)`: the residents the iteration serves.
    pub only: Option<Expr>,
    /// Pool whose holdings of the scheduled residents give `kv_decode`.
    pub memory: Option<Ref>,
}

/// `serve` of a step stage: one order, where two booleans (`exclusive
/// prefill`, `decode first`) used to describe it and could both be set.
#[derive(Clone, Debug, PartialEq)]
pub enum Serve {
    /// Admission order (vLLM's `running` list). The default.
    Admission,
    /// The decoding residents before the prefilling ones (the paper's
    /// two-resource replica: prefill gets what decode leaves). Sugar: the
    /// linker writes it as `by (decoding ? 0 : 1)`.
    DecodeFirst,
    /// Ascending keys over the residents (`decoding`, `admitted`,
    /// `remaining`, `now`), ties in admission order.
    By(Vec<Expr>),
    /// Only the first prefilling resident while one exists; every decode
    /// stalls (the RBLN stack).
    ExclusivePrefill,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StageDecl {
    pub span: Option<Span>,
    pub name: String,
    pub count: usize,
    /// Declared as an array (`stage E[N]`): its members carry their index,
    /// a one-member array's too.
    pub array: bool,
    pub kind: StageKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Arrival {
    Poisson(Expr),
    /// Renewal arrivals with independently sampled interarrival times.
    Renewal(Expr),
    /// `n` sessions always live: an ended one is replaced at once.
    Closed(Expr),
    /// `n` sessions at time 0, never replaced.
    Batch(Expr),
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Workload {
    pub arrive: Arrival,
    pub trace: Option<String>,
    /// Sessions take the trace's sessions in order (session i = arrival i)
    /// instead of a uniform draw.
    pub trace_ordered: bool,
    pub init: Vec<Stmt>,
    pub turn: Vec<Stmt>,
    /// Attributes the scheduler may not read (`hidden o;`): legal in
    /// session statements, rejected at every other moment (a hold's header, a queue or
    /// eviction key, a spill clause, a ps capacity, a step stage's budget, cost, chunk or serve keys).
    pub hidden: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    /// Draw the next turn's attributes from the workload.
    Turn,
    /// `request;` in a workload's `session`: the request runs the `server`
    /// block. Parse-time only: the parser splices the server's statements
    /// in its place before it returns, so the linker never sees one.
    Request,
    Set(String, Expr),
    Observe(String, Expr),
    Hold {
        /// (pool, units allocated, units that must fit for the admission)
        pools: Vec<(Ref, Expr, Option<Expr>)>,
        /// `reuse (r)`: consume at most `r` units of the own cached prefix;
        /// the rest stays cached, unusable, until evicted.
        reuse: Option<Expr>,
        body: Vec<Stmt>,
        cache: Option<Expr>,
        /// `lease P (t)`: at the scope's end the allocation on `P` (one of
        /// the hold's pools) stays, neither evictable nor a preemption
        /// victim, until a `release`/`transfer … from P` of this session
        /// takes it, `t` seconds pass, or the session ends; then `cache`
        /// applies.
        lease: Option<(Ref, Expr)>,
    },
    Grow(Ref, Expr),
    Drop(Ref),
    /// `release P;`: the innermost enclosing hold gives its allocation on
    /// `P` back now, caching per its clause.
    Release(Ref),
    /// `load P (n);`: the KV of `n` tokens arrived from outside the engine;
    /// the innermost enclosing hold's computed position on `P` advances.
    Load(Ref, Expr),
    Run {
        stage: Ref,
        mode: RunMode,
        work: Expr,
        growing: Option<Ref>,
        /// `run a, b (w)`: the further stages the job holds at once.
        also: Vec<Ref>,
    },
    Branch(Expr, Vec<Stmt>, Vec<Stmt>),
    Loop(Vec<Stmt>),
    /// `choose j in 0..n by (k1, …)`: `j := argmin`, keys in order.
    Choose {
        var: String,
        count: Expr,
        key: Vec<Expr>,
    },
    End,
    /// `Q[i].verb (args) [from S[k]] [to P (m)];`: a queue's entry. Parse-time
    /// only: `assemble` puts the entry's body in its place (`crate::frontend::queue`),
    /// so the linker never sees one.
    Call {
        queue: Ref,
        verb: String,
        args: Vec<Expr>,
        from: Option<Ref>,
        to: Option<(Ref, Expr)>,
    },
    /// `mark x;` inside a queue's entry: the moment, as the attribute `Q.x`
    /// the caller reads. Parse-time only, as `Call`.
    Mark(String),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct RunOpts {
    pub horizon: Option<Expr>,
    pub warmup: Option<Expr>,
    pub seed: Option<Expr>,
    /// Maximum open-population arrivals, followed by draining their sessions.
    pub arrivals: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Program {
    /// Locations of constants and assigned attributes, for diagnostic notes.
    pub definitions: Vec<(String, Span)>,
    /// The libraries `use` read, for the spans that point into them.
    pub libs: Vec<crate::frontend::diagnostic::Source>,
    pub lets: Vec<(String, Expr)>,
    pub pools: Vec<PoolDecl>,
    pub stages: Vec<StageDecl>,
    pub workload: Option<Workload>,
    pub session: Vec<Stmt>,
    /// What one request runs, as the session runs it at its `request`: the
    /// server, or a gateway's `route`, expanded like the session. Empty when
    /// the program does not split its session into a workload and a server,
    /// or requests more than one thing. The deployment view draws this; the
    /// session is what runs.
    pub request: Vec<Stmt>,
    pub run: RunOpts,
    /// `share maxmin;` or `share bottleneck;`
    pub share: Option<crate::ir::Share>,
    /// `gauge NAME = e;`, in order.
    pub gauges: Vec<(String, Expr)>,
    /// `claim NAME …;`, in order.
    pub claims: Vec<ClaimDecl>,
}

/// `claim NAME [given (e)] : every iteration of STAGE (e) ;`, `… some
/// iteration of STAGE (e) ;` or `… at end (e) ;`.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaimDecl {
    pub span: Option<Span>,
    pub name: String,
    pub given: Option<Expr>,
    pub over: ClaimOver,
    pub expr: Expr,
}

/// What a claim quantifies over.
#[derive(Clone, Debug, PartialEq)]
pub enum ClaimOver {
    Every(Ref),
    Some(Ref),
    End,
}

/// Parser equivalence tests compare syntax after desugaring; the two spellings
/// necessarily have different source locations. Diagnostic tests check those
/// locations separately.
#[cfg(test)]
pub(crate) fn without_locations(mut p: Program) -> Program {
    fn expr(e: &mut Expr) {
        while let Expr::Located(_, inner) = e {
            *e = *inner.clone();
        }
        match e {
            Expr::Sample(_, args) => args.iter_mut().for_each(expr),
            Expr::Call(_, args) => args.iter_mut().for_each(|a| match a {
                Arg::Expr(e) => expr(e),
                Arg::Ref(r) => reference(r),
            }),
            Expr::Unary(_, e) => expr(e),
            Expr::Binary(_, a, b) => {
                expr(a);
                expr(b);
            }
            Expr::Cond(a, b, c) => {
                expr(a);
                expr(b);
                expr(c);
            }
            Expr::Over(_, _, n, e) => {
                expr(n);
                expr(e);
            }
            _ => {}
        }
    }
    fn reference(r: &mut Ref) {
        r.span = None;
        if let Some(e) = &mut r.index {
            expr(e);
        }
    }
    fn block(stmts: &mut [Stmt]) {
        for s in stmts {
            match s {
                Stmt::Set(_, e) | Stmt::Observe(_, e) => expr(e),
                Stmt::Hold {
                    pools,
                    reuse,
                    body,
                    cache,
                    lease,
                } => {
                    for (r, e, reserve) in pools {
                        reference(r);
                        expr(e);
                        reserve.iter_mut().for_each(expr);
                    }
                    reuse.iter_mut().for_each(expr);
                    cache.iter_mut().for_each(expr);
                    if let Some((r, duration)) = lease {
                        reference(r);
                        expr(duration);
                    }
                    block(body);
                }
                Stmt::Grow(r, e) | Stmt::Load(r, e) => {
                    reference(r);
                    expr(e);
                }
                Stmt::Drop(r) | Stmt::Release(r) => reference(r),
                Stmt::Run {
                    stage,
                    work,
                    growing,
                    also,
                    ..
                } => {
                    reference(stage);
                    also.iter_mut().for_each(reference);
                    expr(work);
                    growing.iter_mut().for_each(reference);
                }
                Stmt::Branch(e, a, b) => {
                    expr(e);
                    block(a);
                    block(b);
                }
                Stmt::Loop(b) => block(b),
                Stmt::Choose { count, key, .. } => {
                    expr(count);
                    key.iter_mut().for_each(expr);
                }
                _ => {}
            }
        }
    }
    p.definitions.clear();
    for (_, e) in &mut p.lets {
        expr(e);
    }
    for d in &mut p.pools {
        d.span = None;
        expr(&mut d.cap);
        d.block.iter_mut().for_each(expr);
        if let EvictOrder::By(keys) = &mut d.evict {
            keys.iter_mut().for_each(expr);
        }
        if let QueueOrder::By(keys) = &mut d.queue {
            keys.iter_mut().for_each(expr);
        }
        if let Some(s) = &mut d.spill {
            reference(&mut s.to);
            reference(&mut s.via);
            expr(&mut s.work);
            expr(&mut s.when);
        }
        d.admit_via.iter_mut().for_each(reference);
    }
    for d in &mut p.stages {
        d.span = None;
        match &mut d.kind {
            StageKind::Fifo(e) | StageKind::Ps(e) => expr(e),
            StageKind::Step(s) => {
                expr(&mut s.budget);
                expr(&mut s.cost);
                expr(&mut s.chunk);
                s.memory.iter_mut().for_each(reference);
                s.only.iter_mut().for_each(expr);
                if let Serve::By(keys) = &mut s.serve {
                    keys.iter_mut().for_each(expr);
                }
            }
            StageKind::Delay => {}
        }
    }
    if let Some(w) = &mut p.workload {
        match &mut w.arrive {
            Arrival::Poisson(e) | Arrival::Renewal(e) | Arrival::Closed(e) | Arrival::Batch(e) => {
                expr(e)
            }
            Arrival::None => {}
        }
        block(&mut w.init);
        block(&mut w.turn);
    }
    block(&mut p.session);
    block(&mut p.request);
    p.run.horizon.iter_mut().for_each(expr);
    p.run.warmup.iter_mut().for_each(expr);
    p.run.seed.iter_mut().for_each(expr);
    p.run.arrivals.iter_mut().for_each(expr);
    for c in &mut p.claims {
        c.span = None;
        c.given.iter_mut().for_each(expr);
        if let ClaimOver::Every(r) | ClaimOver::Some(r) = &mut c.over {
            reference(r);
        }
        expr(&mut c.expr);
    }
    p
}
