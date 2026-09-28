//! Abstract syntax of a seQ program.
//!
//! A program is a *deployment* (pools and stages), a *workload* (how
//! sessions arrive and how a session's attributes evolve from turn to
//! turn) and a *session* (the statements every session executes). A
//! program written as `workload { … session { … request; … } }` and
//! `server { … }` arrives here with the server spliced into the session:
//! the split is the parser's. See `docs/language.md` for the semantics.

pub use crate::ir::{BinOp, Preempt, RunMode, UnOp};

/// Expressions are evaluated to `f64`. Booleans are 0 / 1.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
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
    pub name: String,
    pub index: Option<Box<Expr>>,
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
    By(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spill {
    pub to: String,
    pub via: String,
    pub work: Expr,
    pub when: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PoolDecl {
    pub name: String,
    pub count: usize,
    pub cap: Expr,
    pub block: Option<Expr>,
    pub evict: EvictOrder,
    pub preempt: Preempt,
    pub queue: QueueOrder,
    pub spill: Option<Spill>,
    /// `admit via STAGE`: the queue is served by the stage's scheduler, at
    /// the start of its iterations, while the iteration has budget left.
    pub admit_via: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StageKind {
    /// `c` servers, one job each at rate 1, FIFO.
    Fifo(Expr),
    /// Processor sharing with capacity `phi(n)`, `n` the jobs present.
    Ps(Expr),
    /// Infinite server: every job at rate 1.
    Delay,
    /// Iterating engine (continuous batching with chunked prefill).
    Step(StepSpec),
}

#[derive(Clone, Debug, PartialEq)]
pub struct StepSpec {
    /// Tokens per iteration (`max_num_batched_tokens`).
    pub budget: Expr,
    /// Seconds per iteration, in `ntok`, `ndec`, `npre`, `nres`, `kvb`.
    pub cost: Expr,
    /// Cap on one request's prefill chunk (`long_prefill_token_threshold`,
    /// 0 = none).
    pub chunk: Expr,
    /// The order the iteration serves its residents in (`serve …;`).
    pub serve: Serve,
    /// Pool whose holdings of the scheduled residents give `kvb`.
    pub memory: Option<String>,
}

/// `serve` of a step stage: one order, where two booleans (`exclusive
/// prefill`, `decode first`) used to describe it and could both be set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Serve {
    /// Admission order (vLLM's `running` list). The default.
    Admission,
    /// The decoding residents before the prefilling ones (the paper's
    /// two-resource replica: prefill gets what decode leaves).
    DecodeFirst,
    /// Only the first prefilling resident while one exists; every decode
    /// stalls (the RBLN stack).
    ExclusivePrefill,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StageDecl {
    pub name: String,
    pub count: usize,
    pub kind: StageKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Arrival {
    Poisson(Expr),
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
    },
    Grow(Ref, Expr),
    Drop(Ref),
    Run {
        stage: Ref,
        mode: RunMode,
        work: Expr,
        growing: Option<Ref>,
    },
    Branch(Expr, Vec<Stmt>, Vec<Stmt>),
    Loop(Vec<Stmt>),
    /// `choose j in 0..n by (expr)`: `j := argmin`.
    Choose {
        var: String,
        count: Expr,
        key: Expr,
    },
    End,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct RunOpts {
    pub horizon: Option<Expr>,
    pub warmup: Option<Expr>,
    pub seed: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Program {
    pub lets: Vec<(String, Expr)>,
    pub pools: Vec<PoolDecl>,
    pub stages: Vec<StageDecl>,
    pub workload: Option<Workload>,
    pub session: Vec<Stmt>,
    pub run: RunOpts,
}
