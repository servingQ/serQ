//! Abstract syntax of a serQ program.
//!
//! A program is a *deployment* (pools and stages), a *workload* (how
//! sessions arrive and how a session's attributes evolve from turn to
//! turn) and a *session* (the statements every session executes). A
//! program written as `workload { … session { … turn; … } }` and
//! `server { … }` arrives here with the server spliced into the session:
//! the split is the parser's. See `docs/language.md` for the semantics.

pub use crate::frontend::diagnostic::Span;

pub use crate::ir::{BinOp, RunMode, UnOp};

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
    /// A definition's use whose body never reads some parameters: those
    /// arguments, checked as if read where the use stands, then the body.
    /// Parse-time only: the linker lowers the body alone.
    Unread(Vec<Arg>, Box<Expr>),
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

impl Arg {
    /// `Expr::any` in a call's argument: a reference's index.
    pub fn any(&self, f: &impl Fn(&Expr) -> bool) -> bool {
        match self {
            Arg::Expr(x) => x.any(f),
            Arg::Ref(r) => r.index.as_ref().is_some_and(|i| i.any(f)),
        }
    }

    /// `Expr::for_each_mut` in a call's argument: a reference's index.
    pub fn for_each_mut(&mut self, f: &mut impl FnMut(&mut Expr)) {
        match self {
            Arg::Expr(x) => x.for_each_mut(f),
            Arg::Ref(r) => r.index.iter_mut().for_each(|i| i.for_each_mut(f)),
        }
    }

    /// `Expr::substitute` in a call's argument.
    pub fn substitute(&mut self, binds: &[(String, Expr)]) {
        match self {
            Arg::Expr(x) => x.substitute(binds),
            Arg::Ref(r) => {
                // a bare identifier argument is parsed as a reference (it
                // may name a pool or a stage); when it names a binding it
                // is the binding, else `min(known, …)` would read the
                // attribute `known` and not the header's `known = …`
                if r.index.is_none()
                    && let Some((_, v)) = binds.iter().find(|(name, _)| *name == r.name)
                {
                    *self = Arg::Expr(v.clone());
                } else if let Some(i) = &mut r.index {
                    i.substitute(binds);
                }
            }
        }
    }
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
    /// Serving vocabulary is a named conversion from request quantities into
    /// work at its stage(s). Primitive `run` requires an explicit conversion.
    pub(crate) fn cost(resources: &[Ref], value: Expr) -> Expr {
        let mut args: Vec<_> = resources
            .iter()
            .cloned()
            .map(|mut r| {
                r.index = None;
                Arg::Ref(r)
            })
            .collect();
        args.push(Arg::Expr(value));
        Expr::Call("cost".into(), args)
    }

    /// Whether `f` holds of this expression or of one in it: the operand of
    /// a `Located`, a reference's index and an aggregate's count and body
    /// included (#280). Names are not resolved: an aggregate's index is not
    /// shadowed, and a bare argument (a reference) is not an `Expr` here, so
    /// a question about what a name reads is not this walk's.
    pub fn any(&self, f: &impl Fn(&Expr) -> bool) -> bool {
        f(self)
            || match self {
                Expr::Num(_) | Expr::Var(_) => false,
                Expr::Located(_, x) | Expr::Unary(_, x) => x.any(f),
                Expr::Sample(_, xs) => xs.iter().any(|x| x.any(f)),
                Expr::Call(_, args) => args.iter().any(|a| a.any(f)),
                // an unread argument is never evaluated
                Expr::Unread(_, x) => x.any(f),
                Expr::Binary(_, a, b) | Expr::Over(_, _, a, b) => a.any(f) || b.any(f),
                Expr::Cond(c, a, b) => c.any(f) || a.any(f) || b.any(f),
            }
    }

    /// `f` on this expression, then on each one in it that `any` visits,
    /// in the same order.
    pub fn for_each_mut(&mut self, f: &mut impl FnMut(&mut Expr)) {
        f(self);
        match self {
            Expr::Num(_) | Expr::Var(_) => {}
            Expr::Located(_, x) | Expr::Unary(_, x) | Expr::Unread(_, x) => x.for_each_mut(f),
            Expr::Sample(_, xs) => xs.iter_mut().for_each(|x| x.for_each_mut(f)),
            Expr::Call(_, args) => args.iter_mut().for_each(|a| a.for_each_mut(f)),
            Expr::Binary(_, a, b) | Expr::Over(_, _, a, b) => {
                a.for_each_mut(f);
                b.for_each_mut(f);
            }
            Expr::Cond(c, a, b) => {
                c.for_each_mut(f);
                a.for_each_mut(f);
                b.for_each_mut(f);
            }
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
            Expr::Call(_, args) => args.iter_mut().for_each(|a| a.substitute(binds)),
            Expr::Unread(args, x) => {
                args.iter_mut().for_each(|a| a.substitute(binds));
                x.substitute(binds);
            }
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

    /// `substitute` into the arguments of each `Unread` in it, and nowhere
    /// else: a hold's body reads a binding as the attribute set at its top,
    /// where an unread argument, which reads nothing, sees the binding.
    pub fn bind_unread(&mut self, binds: &[(String, Expr)]) {
        self.for_each_mut(&mut |x| {
            if let Expr::Unread(args, _) = x {
                args.iter_mut().for_each(|a| a.substitute(binds));
            }
        });
    }

    /// Whether the expression draws (`~`) anywhere.
    pub fn draws(&self) -> bool {
        self.any(&|x| matches!(x, Expr::Sample(..)))
    }

    pub(crate) fn same_syntax(&self, other: &Self) -> bool {
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

/// What a growth that does not fit does (`preempt …`).
#[derive(Clone, Debug, PartialEq)]
pub enum PreemptOrder {
    /// `preempt none`: the grower waits.
    None,
    /// `preempt by (k, …) [requeue head | requeue tail]`: the candidate
    /// with the least keys is the victim; `tail` re-queues it as a
    /// newcomer. `preempt lifo` is `By { keys: [-admission], tail: false }`.
    By { keys: Vec<Expr>, tail: bool },
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
    pub preempt: PreemptOrder,
    pub queue: QueueOrder,
    pub spill: Option<Spill>,
    /// `admit via STAGE`: the queue is served by the stage's scheduler, at
    /// the start of its iterations, while the iteration has budget left.
    pub admit_via: Option<Ref>,
    /// `reserve held`: a hold's unallocated reservation counts against
    /// later admissions while it lasts.
    pub reserve_held: bool,
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
    /// An engine's `each at most`: the cap on one request's prefill chunk
    /// (`long_prefill_token_threshold`). An outcome at or below 0 does not
    /// link, and `inf` is no cap, which the linker writes 0.
    pub chunk: Expr,
    /// `granule g`: a prefill gets all it has left or a multiple of `g`.
    pub granule: Option<Expr>,
    /// The order the iteration serves its residents in: `advance running`'s
    /// order, or `exclusive prefill`.
    pub serve: Serve,
    /// Pool whose holdings of the scheduled residents give `kv_decode`.
    pub memory: Option<Ref>,
    /// The engine's `schedule`, as the kernel runs it.
    pub schedule: Schedule,
    /// `state NAME = c;`: the stage's registers and their first values.
    pub state: Vec<(String, Expr)>,
}

/// An engine's `schedule` as the kernel runs it.
#[derive(Clone, Debug, PartialEq)]
pub enum Schedule {
    /// vLLM's procedure, no body: advance running in `serve`'s order, then
    /// admit waiting while nothing was preempted; `Some(p)` is the `only (p)`
    /// both statements share.
    Procedure(Option<Expr>),
    /// Any other schedule, as its statements.
    Body(Vec<IterStmt>),
}

/// A statement of an engine's `schedule` body, as the kernel runs it.
#[derive(Clone, Debug, PartialEq)]
pub enum IterStmt {
    /// `serve [only (p)] [admission | decode first | by (k, …)];`
    Serve {
        only: Option<Expr>,
        order: Option<Serve>,
    },
    /// `admit [only (p)] [while (e)];`
    Admit {
        only: Option<Expr>,
        gate: Option<Expr>,
    },
    /// `branch (e) { … } [else { … }]`
    Branch(Expr, Vec<IterStmt>, Vec<IterStmt>),
    /// `set NAME = e;`: one of the stage's registers.
    Set(String, Expr),
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

/// Source declarations are checked by the linker and erased before execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclaredType {
    Size,
    Cost,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    /// Parse-time boundary, retained as per-statement authority in the IR.
    Side(crate::ir::Side),
    Declare(String, DeclaredType),
    /// Draw the next turn's attributes from the workload.
    Turn,
    /// Internal marker after a source `turn;`: splice the `server` block.
    /// Parse-time only: the parser splices the server's statements
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
    While(Expr, Vec<Stmt>),
    /// `choose j in 0..n by (k1, …)`: `j := argmin`, keys in order.
    Choose {
        var: String,
        count: Expr,
        key: Vec<Expr>,
    },
    End,
    /// `fork { … }`: the block runs beside the session, as a leg of the
    /// same request (`CStmt::Fork`).
    Fork(Vec<Stmt>),
    /// `join;`: wait until every leg forked so far has ended.
    Join,
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
    /// The arguments a statement definition's use passes to parameters
    /// its body never reads (`Expr::Unread`). The linker resolves them and
    /// lowers the statement to nothing.
    Unread(Vec<Arg>),
}

/// What `each_part_mut` hands a visitor: each expression a statement holds,
/// each pool, stage or queue it names, and each argument of a
/// `Stmt::Unread`. What lies inside one (a reference's index, a nested
/// expression) is the visitor's to walk.
pub(crate) struct Parts<E, R, A> {
    pub expr: E,
    pub reference: R,
    pub unread: A,
}

/// Hand every part of `stmts` to `parts`, a nested block's statements
/// included, so that a walk over statements cannot pass over a place an
/// expression stands: a reference's index is one (`hold g[n]`).
pub(crate) fn each_part_mut<E, R, A>(stmts: &mut [Stmt], parts: &mut Parts<E, R, A>)
where
    E: FnMut(&mut Expr),
    R: FnMut(&mut Ref),
    A: FnMut(&mut Arg),
{
    for s in stmts {
        match s {
            Stmt::Side(_)
            | Stmt::Declare(..)
            | Stmt::Turn
            | Stmt::Request
            | Stmt::End
            | Stmt::Join
            | Stmt::Mark(_) => {}
            Stmt::Set(_, e) | Stmt::Observe(_, e) => (parts.expr)(e),
            Stmt::Unread(args) => args.iter_mut().for_each(&mut parts.unread),
            Stmt::Hold {
                pools,
                reuse,
                body,
                cache,
                lease,
            } => {
                for (r, e, reserve) in pools {
                    (parts.reference)(r);
                    (parts.expr)(e);
                    reserve.iter_mut().for_each(&mut parts.expr);
                }
                reuse.iter_mut().for_each(&mut parts.expr);
                cache.iter_mut().for_each(&mut parts.expr);
                if let Some((r, duration)) = lease {
                    (parts.reference)(r);
                    (parts.expr)(duration);
                }
                each_part_mut(body, parts);
            }
            Stmt::Grow(r, e) | Stmt::Load(r, e) => {
                (parts.reference)(r);
                (parts.expr)(e);
            }
            Stmt::Drop(r) | Stmt::Release(r) => (parts.reference)(r),
            Stmt::Run {
                stage,
                work,
                growing,
                also,
                ..
            } => {
                (parts.reference)(stage);
                also.iter_mut().for_each(&mut parts.reference);
                (parts.expr)(work);
                growing.iter_mut().for_each(&mut parts.reference);
            }
            Stmt::Branch(e, a, b) => {
                (parts.expr)(e);
                each_part_mut(a, parts);
                each_part_mut(b, parts);
            }
            Stmt::While(e, b) => {
                (parts.expr)(e);
                each_part_mut(b, parts);
            }
            Stmt::Loop(b) | Stmt::Fork(b) => each_part_mut(b, parts),
            Stmt::Choose { count, key, .. } => {
                (parts.expr)(count);
                key.iter_mut().for_each(&mut parts.expr);
            }
            Stmt::Call {
                queue,
                args,
                from,
                to,
                ..
            } => {
                (parts.reference)(queue);
                args.iter_mut().for_each(&mut parts.expr);
                from.iter_mut().for_each(&mut parts.reference);
                if let Some((r, e)) = to {
                    (parts.reference)(r);
                    (parts.expr)(e);
                }
            }
        }
    }
}

/// Execution settings parsed only from an external instance, never a model.
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
    /// The source declares its executable entry point. Libraries have none.
    pub has_main: bool,
    /// (external argument name, declaration index in `lets`), from std/args.
    /// A name can be shadowed; inputs belong to declarations, not spellings.
    pub inputs: Vec<(String, usize)>,
    /// Locations of constants and assigned attributes, for diagnostic notes.
    pub definitions: Vec<(String, Span)>,
    /// Composite Cost names and resource fields; source-only namespace data.
    pub cost_records: Vec<(String, Vec<String>)>,
    /// The libraries `use` read, for the spans that point into them.
    pub libs: Vec<crate::frontend::diagnostic::Source>,
    pub lets: Vec<(String, Expr)>,
    pub pools: Vec<PoolDecl>,
    pub stages: Vec<StageDecl>,
    pub workload: Option<Workload>,
    pub session: Vec<Stmt>,
    /// What one request runs, as the session runs it at its `request`: the
    /// server, or a gateway's `route`, expanded like the session. Empty when
    /// no request body exists, or the session requests more than one thing.
    /// The deployment view draws this; the session is what runs.
    pub request: Vec<Stmt>,
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
            Expr::Call(_, args) => args.iter_mut().for_each(arg),
            Expr::Unread(args, e) => {
                args.iter_mut().for_each(arg);
                expr(e);
            }
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
    fn arg(a: &mut Arg) {
        match a {
            Arg::Expr(e) => expr(e),
            Arg::Ref(r) => reference(r),
        }
    }
    fn reference(r: &mut Ref) {
        r.span = None;
        if let Some(e) = &mut r.index {
            expr(e);
        }
    }
    fn block(stmts: &mut [Stmt]) {
        each_part_mut(
            stmts,
            &mut Parts {
                expr,
                reference,
                unread: arg,
            },
        );
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
                match &mut s.schedule {
                    Schedule::Procedure(only) => only.iter_mut().for_each(expr),
                    // a body's statements keep their locations: a test of
                    // equal programs compares procedures, not bodies
                    Schedule::Body(_) => {}
                }
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
