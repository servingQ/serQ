//! Abstract syntax of a serQ program.
//!
//! A program is a *deployment* (pools and stages), a *workload* (how
//! sessions arrive and how a session's attributes evolve from turn to
//! turn) and a *session* (the statements every session executes). A
//! program written as `workload { … session { … turn; … } }` and
//! `server { … }` arrives here with the server spliced into the session:
//! the split is the parser's. See `docs/language.md` for the semantics.

pub use crate::frontend::diagnostic::Span;

pub use crate::ir::{ArgKind, BinOp, Fun, RunMode, UnOp};

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

    /// `Expr::any_written` in a call's argument: a reference's index.
    pub fn any_written(&self, f: &impl Fn(&Expr) -> bool) -> bool {
        match self {
            Arg::Expr(x) => x.any_written(f),
            Arg::Ref(r) => r.index.as_ref().is_some_and(|i| i.any_written(f)),
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

/// Does `f` take a number at argument `k` of `n`? There a bare name is a
/// value, as the linker resolves it: `min(o, 1)` reads `o` as
/// `min(o + 0, 1)` does. Elsewhere it names a pool, a stage or an observe
/// (`queued(kv)`, `total(latency)`). The function's signature says which,
/// and `cost`'s last argument is the quantity it converts.
pub fn takes_number(f: &str, k: usize, n: usize) -> bool {
    if f == "cost" {
        return k + 1 == n;
    }
    Fun::from_name(f).is_some_and(|fun| fun.signature().get(k) == Some(&ArgKind::Expr))
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
        // a bare name as the kernel text parses it, `cost(E, S)`, so that
        // the linker reads it as any argument that `takes_number`
        let bare = |span, name| {
            Arg::Ref(Ref {
                span,
                name,
                index: None,
            })
        };
        args.push(match value {
            Expr::Var(name) => bare(None, name),
            Expr::Located(span, inner) => match *inner {
                Expr::Var(name) => bare(Some(span), name),
                inner => Arg::Expr(Expr::Located(span, Box::new(inner))),
            },
            value => Arg::Expr(value),
        });
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

    /// `any`, and in the arguments a call does not read too
    /// (`Expr::Unread`): what is written, evaluated or not. A question
    /// about the text, such as whether an aggregate's index is a name of
    /// its own, is this walk's; one about the value, such as whether it
    /// draws, is `any`'s.
    pub fn any_written(&self, f: &impl Fn(&Expr) -> bool) -> bool {
        self.any(&|x| {
            f(x) || matches!(x, Expr::Unread(args, _) if args.iter().any(|a| a.any_written(f)))
        })
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

    /// Replace every `Var(name)` of `binds` by its expression: an
    /// aggregate's index (`max j in n`) by its number, say. A bare argument
    /// naming one is the binding, not a reference; an inner aggregate's
    /// index is its own.
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
    /// `preempt leases by (k, …) [every (dt) [suspend by (k, …)]]`: the
    /// lease with the least keys ends, or at the ticks only, or is set
    /// aside there and given back in the `suspend` keys' order.
    Leases {
        keys: Vec<Expr>,
        every: Option<Expr>,
        suspend: Option<Vec<Expr>>,
    },
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
    /// `queue … every (dt)`: admissions at the pool's ticks only.
    pub queue_every: Option<Expr>,
    pub spill: Option<Spill>,
    /// The engine whose scheduler serves the queue (`pool X on E`, `on E.DEVICE`), at
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
    /// The engine's `scheduler`, as the kernel runs it.
    pub schedule: Schedule,
    /// `state NAME = c;`: the stage's registers and their first values.
    pub state: Vec<(String, Expr)>,
    /// What its admission takes of a request and what the release leaves
    /// cached (`taking` on `admit waiting`); `None` before the engine is read.
    pub request: Option<RequestSpec>,
    /// `connector C { … }`: its KV connector to the L2 queue `C`.
    pub connector: Option<ConnectorSpec>,
}

/// An engine's `connector C { per_token (r); [load (n);] [save (n);] }`,
/// read for each request (`ir::CConnector`).
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectorSpec {
    /// The queue it names, as written.
    pub queue: String,
    pub kind: ConnectorKind,
    /// The tokens a request loads at its admission.
    pub load: Option<Expr>,
    /// The tokens the L2 holds of a request once it has ended.
    pub save: Option<Expr>,
}

/// What a connector copies through.
#[derive(Clone, Debug, PartialEq)]
pub enum ConnectorKind {
    /// An L2 queue (`queue C : l2`): its stage, whose bandwidth the copies
    /// share, its pool (`C.kv`), and the time one token's copy takes;
    /// `link`, the engine's own link its copies also cross, when
    /// `kvt.l2_link (C, Q, …)` declares one (#600).
    L2 {
        stage: Ref,
        pool: Ref,
        per_token: Expr,
        link: Option<L2Link>,
    },
    /// The source of the KV relation of the engine's queue (`kvt.nixl_pull
    /// (C, Q, …)`, `kvt.nixl_push`): a load reads, from the lease the
    /// request's entry took `from C`, over the two NICs. `None` until the
    /// program is read, where the relation may be declared below.
    Relation(Option<Relation>),
}

/// `kvt.l2_link (C, Q, share = s);`: the copies of `Q`'s engine's
/// `connector C`, its loads and its saves, hold `C`'s `serve` and `Q`'s NIC
/// at once, divided by `s`.
#[derive(Clone, Debug, PartialEq)]
pub struct L2Link {
    /// The engine's queue's NIC, `Q.nic`.
    pub nic: String,
    pub share: crate::ir::Share,
}

/// A connector's KV relation, as `kvt.nixl_pull` or `kvt.nixl_push`
/// declared it.
#[derive(Clone, Debug, PartialEq)]
pub struct Relation {
    /// The source queue, and the pool its entries lease (`P.kv`).
    pub source: String,
    pub leased: String,
    /// The engine's queue, which reads.
    pub reader: String,
    /// The wait before each copy: the copy's run's `latency`, the poster's.
    pub latency: Option<Expr>,
    pub push: bool,
    pub share: crate::ir::Share,
    /// `ttl = t`: how long a send the reader does not read keeps the
    /// source's blocks (`ir::CSend::ttl`); absent, until the session's end.
    pub ttl: Option<f64>,
}

/// An engine's `taking P (u) [reserve (r)], … [reuse (e)] [prefix (…)]
/// [cache (e)]`, read for each request (`ir::CRequest`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RequestSpec {
    pub pools: Vec<(Ref, Expr, Option<Expr>)>,
    pub reuse: Option<Expr>,
    pub prefix: Option<Vec<(Expr, Expr)>>,
    pub cache: Option<Expr>,
}

/// An engine's `scheduler` as the kernel runs it.
#[derive(Clone, Debug, PartialEq)]
pub enum Schedule {
    /// vLLM's procedure, no body: advance running in `serve`'s order, then
    /// admit waiting while nothing was preempted; `Some(p)` is the `only (p)`
    /// both statements share.
    Procedure(Option<Expr>),
    /// Any other schedule, as its statements.
    Body(Vec<IterStmt>),
}

/// A statement of an engine's `scheduler` body, as the kernel runs it.
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
    /// `if (e) { … } [else { … }]`
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
    /// Attributes the scheduler may not read, the server's hidden
    /// parameters (`server (…, hidden o)`): legal in
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

/// Where a gateway's `route`, expanded in place, opens and closes.
#[derive(Clone, Debug, PartialEq)]
pub enum RouteMark {
    /// The route of the gateway so named starts after it.
    Open(String),
    /// The route opened last ends before it.
    Close,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    /// Parse-time boundary, retained as per-statement authority in the IR.
    Side(crate::ir::Side),
    /// Parse-time boundary of a gateway's `route` expanded in place.
    /// Retained in the IR as `Program::routes`.
    Route(RouteMark),
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
        /// `prefix (k₁: n₁, …)`: the hold's sequence as keyed segments.
        prefix: Option<Vec<(Expr, Expr)>>,
        /// `lease P (t)`: at the scope's end the allocation on `P` (one of
        /// the hold's pools) stays, neither evictable nor a preemption
        /// victim, until a `release`/`transfer … from P` of this session
        /// takes it, `t` seconds pass, or the session ends; then `cache`
        /// applies.
        lease: Option<(Ref, Expr)>,
    },
    Grow(Ref, Expr),
    /// `shrink P (e);`: give back `e` units of the hold's allocation, or of the lease
    Shrink(Ref, Expr),
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
        /// `run a, b (w, share = share.maxmin)`: the further stages the job
        /// holds at once.
        also: Vec<Ref>,
        /// How the flows on `also`'s fabric divide it: given exactly when
        /// `also` is not empty.
        share: Option<crate::ir::Share>,
        /// The wait before the work starts, in seconds: a link's `serve …
        /// latency`, a relation's `latency = …` (`CStmt::Run::latency`).
        latency: Option<Expr>,
        /// Which of the run's stages posts the copy, for a KV relation's
        /// (`CStmt::Run::poster`); `None` for a run written in the text.
        poster: Option<usize>,
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
    /// same turn (`CStmt::Fork`).
    Fork(Vec<Stmt>),
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
    /// `E (input, output);`: a request to engine `E` (`ir::CStmt::Request`).
    EngineRequest {
        stage: Ref,
        input: Expr,
        output: Expr,
    },
    /// The arguments a statement definition's use passes to parameters
    /// its body never reads (`Expr::Unread`). The linker resolves them and
    /// lowers the statement to nothing.
    Unread(Vec<Arg>),
}

/// The blocks a statement holds, as a pair of the first and the second (an
/// `if`'s else-arm): the one list of them, which `Stmt::blocks` and
/// `Stmt::blocks_mut` both expand, the match binding `&` or `&mut` as the
/// statement is borrowed. It names every statement, so a new statement that
/// holds a block does not compile until it is added here, once (#280).
macro_rules! blocks_of {
    ($s:expr) => {
        match $s {
            Stmt::Hold { body, .. }
            | Stmt::Loop(body)
            | Stmt::While(_, body)
            | Stmt::Fork(body) => (Some(body), None),
            Stmt::Branch(_, a, b) => (Some(a), Some(b)),
            Stmt::Side(_)
            | Stmt::Route(_)
            | Stmt::Declare(..)
            | Stmt::Turn
            | Stmt::Request
            | Stmt::Set(..)
            | Stmt::Observe(..)
            | Stmt::Grow(..)
            | Stmt::Shrink(..)
            | Stmt::Drop(_)
            | Stmt::Release(_)
            | Stmt::Load(..)
            | Stmt::Run { .. }
            | Stmt::Choose { .. }
            | Stmt::End
            | Stmt::Call { .. }
            | Stmt::EngineRequest { .. }
            | Stmt::Unread(_) => (None, None),
        }
    };
}

impl Stmt {
    /// The blocks this statement holds, in source order: a hold's body, an
    /// `if`'s then-arm and then its else-arm, a `loop`'s, a `while`'s and a
    /// `fork`'s body. A walk that reaches every statement wherever it stands
    /// descends through this (or `blocks_mut`), not a match of its own.
    pub fn blocks(&self) -> impl Iterator<Item = &[Stmt]> {
        let (a, b) = blocks_of!(self);
        a.into_iter().chain(b).map(Vec::as_slice)
    }

    /// `blocks`, to rewrite: the same blocks in the same order, each a
    /// `Vec` so that a walk may replace a statement by several.
    pub fn blocks_mut(&mut self) -> impl Iterator<Item = &mut Vec<Stmt>> {
        let (a, b) = blocks_of!(self);
        a.into_iter().chain(b)
    }
}

/// What `each_part_mut` hands a visitor: each expression a statement holds,
/// each pool, stage or queue it names, and each argument of a
/// `Stmt::Unread`. What lies inside one (a reference's index, a nested
/// expression) is the visitor's to walk. Only `without_locations`, a test
/// helper, walks statements this way.
#[cfg(test)]
pub(crate) struct Parts<E, R, A> {
    pub expr: E,
    pub reference: R,
    pub unread: A,
}

/// Hand every part of `stmts` to `parts`, a statement's own before those
/// of the blocks it holds (`Stmt::blocks_mut`), so that `without_locations`
/// cannot pass over a place an expression stands: a reference's index is
/// one (`hold g[n]`).
#[cfg(test)]
pub(crate) fn each_part_mut<E, R, A>(stmts: &mut [Stmt], parts: &mut Parts<E, R, A>)
where
    E: FnMut(&mut Expr),
    R: FnMut(&mut Ref),
    A: FnMut(&mut Arg),
{
    for s in stmts {
        match s {
            Stmt::Side(_)
            | Stmt::Route(_)
            | Stmt::Declare(..)
            | Stmt::Turn
            | Stmt::Request
            | Stmt::End
            | Stmt::Loop(_)
            | Stmt::Fork(_) => {}
            Stmt::Set(_, e) | Stmt::Observe(_, e) => (parts.expr)(e),
            Stmt::Unread(args) => args.iter_mut().for_each(&mut parts.unread),
            Stmt::Hold {
                pools,
                reuse,
                body: _,
                cache,
                prefix,
                lease,
            } => {
                for (r, e, reserve) in pools {
                    (parts.reference)(r);
                    (parts.expr)(e);
                    reserve.iter_mut().for_each(&mut parts.expr);
                }
                for (k, n) in prefix.iter_mut().flatten() {
                    (parts.expr)(k);
                    (parts.expr)(n);
                }
                reuse.iter_mut().for_each(&mut parts.expr);
                cache.iter_mut().for_each(&mut parts.expr);
                if let Some((r, duration)) = lease {
                    (parts.reference)(r);
                    (parts.expr)(duration);
                }
            }
            Stmt::Grow(r, e) | Stmt::Shrink(r, e) | Stmt::Load(r, e) => {
                (parts.reference)(r);
                (parts.expr)(e);
            }
            Stmt::Drop(r) | Stmt::Release(r) => (parts.reference)(r),
            Stmt::EngineRequest {
                stage,
                input,
                output,
            } => {
                (parts.reference)(stage);
                (parts.expr)(input);
                (parts.expr)(output);
            }
            Stmt::Run {
                stage,
                work,
                growing,
                also,
                latency,
                ..
            } => {
                (parts.reference)(stage);
                also.iter_mut().for_each(&mut parts.reference);
                (parts.expr)(work);
                latency.iter_mut().for_each(&mut parts.expr);
                growing.iter_mut().for_each(&mut parts.reference);
            }
            Stmt::Branch(e, ..) | Stmt::While(e, _) => (parts.expr)(e),
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
        for b in s.blocks_mut() {
            each_part_mut(b, parts);
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
    /// The source declares its executable entry point.
    pub has_main: bool,
    /// (external argument name, declaration index in `lets`), from std/args.
    /// A name can be shadowed; inputs belong to declarations, not spellings.
    pub inputs: Vec<(String, usize)>,
    /// Locations of constants and assigned attributes, for diagnostic notes.
    pub definitions: Vec<(String, Span)>,
    /// Composite Cost names and resource fields; source-only namespace data.
    pub cost_records: Vec<(String, Vec<String>)>,
    pub lets: Vec<(String, Expr)>,
    /// The constants the parser folds and keeps no expression of (an array
    /// size, a relation's `ttl`) that hold an aggregate. The value is the
    /// parser's; the linker asks each aggregate's index of the whole
    /// program, as it does in an expression (#561).
    pub folded: Vec<Expr>,
    pub pools: Vec<PoolDecl>,
    pub stages: Vec<StageDecl>,
    pub workload: Option<Workload>,
    pub session: Vec<Stmt>,
    /// The `server` block, expanded like the session, which splices it in
    /// at each `request`: the linker checks what it reveals of the
    /// workload's `hidden` attributes. Empty without a `server`.
    pub server: Vec<Stmt>,
    /// `gauge NAME = e;`, in order.
    pub gauges: Vec<(String, Expr)>,
    /// `claim NAME …;`, in order.
    pub claims: Vec<ClaimDecl>,
    /// What crosses between the workload and the server; `None` without a
    /// workload or a server.
    pub interface: Option<Interface>,
    /// The stages and pools a paired workload file declares
    /// (`frontend::paired`): the workload runs on and holds these, and the
    /// deployment none of them.
    /// `None` for one file.
    pub paired: Option<Vec<String>>,
}

/// The attributes each side keeps to itself
/// (`docs/design/request-interface.md`): the linker refuses a read of one
/// from the other side. The parameters and the reply are what crosses.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Interface {
    /// `server (P, …)`, in order.
    pub params: Vec<String>,
    /// `reply (R, …)`, in order; the workload reads each as `reply.R`.
    pub reply: Vec<String>,
    /// What the workload sets and does not send: its own state, which the
    /// server may not read.
    pub client: Vec<String>,
    /// What the workload may read: its own state, what it sends as it is,
    /// and the session's number; no other attribute.
    pub workload: Vec<String>,
}

/// `claim NAME [given (e)] : every iteration of ENGINE (e) ;`, `… some
/// iteration of ENGINE (e) ;` or `… at end (e) ;`.
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
    p.folded.iter_mut().for_each(expr);
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
    block(&mut p.server);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Each statement that holds a block hands its blocks in source order,
    /// the same through `blocks` and `blocks_mut`, and a statement that holds
    /// none hands none.
    #[test]
    fn a_statement_hands_its_blocks_in_source_order() {
        let block = |n: &str| vec![Stmt::Set(n.into(), Expr::Num(0.0))];
        let cases = [
            (
                Stmt::Hold {
                    pools: vec![],
                    reuse: None,
                    body: block("h"),
                    cache: None,
                    prefix: None,
                    lease: None,
                },
                vec!["h"],
            ),
            (
                Stmt::Branch(Expr::Num(1.0), block("then"), block("else")),
                vec!["then", "else"],
            ),
            (Stmt::Loop(block("l")), vec!["l"]),
            (Stmt::While(Expr::Num(1.0), block("w")), vec!["w"]),
            (Stmt::Fork(block("f")), vec!["f"]),
            (Stmt::Set("x".into(), Expr::Num(0.0)), vec![]),
        ];
        fn names<'a>(blocks: impl Iterator<Item = &'a [Stmt]>) -> Vec<String> {
            blocks
                .flat_map(|b| {
                    b.iter().map(|s| match s {
                        Stmt::Set(n, _) => n.clone(),
                        s => panic!("not a block of the test: {s:?}"),
                    })
                })
                .collect()
        }
        for (mut s, want) in cases {
            assert_eq!(names(s.blocks()), want, "blocks of {s:?}");
            let by_mut = names(s.blocks_mut().map(|b| &b[..]));
            assert_eq!(by_mut, want, "blocks_mut of {s:?}");
        }
    }
}
