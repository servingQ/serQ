//! The serQ intermediate representation (IR).
//!
//! The IR is the definition of a serQ program: the interpreter (`engine::interp`) runs
//! it, the Lean model is generated from it, and tools build or edit it as
//! data. The text syntax (`frontend`) is one frontend that compiles
//! to it. The format is serialised as JSON (`Program::to_json`), carries a
//! version, and is checked on load (`Program::validate`). See `docs/ir.md`.

pub mod trace;
mod types;
mod whole;

use serde::{Deserialize, Serialize};

/// Version of the IR meaning; see `docs/ir.md` for the tagged-version policy.
/// 2 added the sessions' turns; 3 renamed `route` to `session`; 4 replaced
/// `CStep`'s `exclusive_prefill` and `decode_first` by `serve`; 5 added
/// `Release` and `Load`; 6 added renewal arrivals and finite open runs; 7
/// makes `Choose.key` a list of keys, compared in order; 8 lets a `Run`
/// hold several stages at once (`also`) under the program's `share` and
/// makes `ExclusivePrefill` isolate the whole batch, including admissions;
/// 9 reevaluates lexicographic queue keys at selection and supplies `Waited`;
/// 10 makes `Hold.cache` the clause that admits a hold to the prefix cache
/// (a hold without it consumes nothing of the session's own entry).
/// 11 separates random streams by session and turn; 12 adds `While`,
/// a guarded loop that continues after its body when the guard becomes zero.
/// IR 12 also adds resource cost conversions and mandatory attribute types
/// and workload/server statement authority. 13 removes `CStageKind::Delay`,
/// which meant `Ps(present)` (`CStageKind::delay`); 13 also adds
/// `Preempt::Leases` (with its `every` and `suspend` keys), a pool's
/// `queue_every`, `Fun::Allocated`,
/// `Shrink`, and `Grow` of a lease outside every hold of its pool
/// (`docs/design/thunderagent.md`), and moves
/// `share` from the program to each `Run` over several stages, one policy
/// per fabric. 14 adds `Run.latency`, a fixed wait before the run's work
/// at its stages, which was a delay stage of its own (`D.nic.latency`,
/// `L.latency`) run before the transfer.
pub const IR_VERSION: u32 = 14;

/// A reason `Program::validate` refuses a program, and the statement it is
/// about (block, index in it) when it is about one.
#[derive(Debug)]
pub struct Invalid {
    pub message: String,
    pub at: Option<(BlockId, usize)>,
    /// The declaration an error about no statement is about (an engine's
    /// clause, a pool's cap), which the text frontend points at.
    pub decl: Option<Decl>,
}

/// A declaration of the deployment, by its index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decl {
    Pool(usize),
    Stage(usize),
}

/// A statement a message quotes as a place, `at ⟨b.k⟩`: `Invalid::placed`
/// writes it as the text's `line:col`, or as the IR's block and statement.
pub(crate) fn place(block: BlockId, index: usize) -> String {
    format!("⟨{block}.{index}⟩")
}

impl Invalid {
    fn at(message: String, block: BlockId, index: usize) -> Self {
        Self {
            message,
            at: Some((block, index)),
            decl: None,
        }
    }

    /// The message, each place it quotes (`place`) written by `at` (the
    /// text's `line:col` of a statement), else as the IR's block and
    /// statement.
    pub fn placed(&self, at: impl Fn(BlockId, usize) -> Option<String>) -> String {
        let mut out = String::new();
        let mut rest = self.message.as_str();
        while let Some(i) = rest.find('⟨') {
            out.push_str(&rest[..i]);
            let tail = &rest[i + '⟨'.len_utf8()..];
            let Some(j) = tail.find('⟩') else {
                out.push_str(&rest[i..]);
                return out;
            };
            let parsed = tail[..j]
                .split_once('.')
                .and_then(|(b, k)| Some((b.parse().ok()?, k.parse().ok()?)));
            match parsed {
                Some((b, k)) => {
                    out.push_str(&at(b, k).unwrap_or_else(|| format!("block {b}, statement {k}")))
                }
                None => out.push_str(&rest[i..i + '⟨'.len_utf8() + j + '⟩'.len_utf8()]),
            }
            rest = &tail[j + '⟩'.len_utf8()..];
        }
        out.push_str(rest);
        out
    }
}

impl From<String> for Invalid {
    fn from(message: String) -> Self {
        Self {
            message,
            at: None,
            decl: None,
        }
    }
}

/// The most sessions a workload may start at once (`closed`, `batch`): each
/// is a state of its own, made before the run begins.
pub const MAX_SESSIONS: usize = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Preempt {
    /// A failed growth waits.
    None,
    /// A failed growth preempts the candidate with the least `keys`
    /// (ascending, read at `Moment::Victim`; ties to the latest admitted),
    /// and the victim's hold re-enters its queue at the head, or at the
    /// tail as a newcomer when `tail`. The candidates are the residents of
    /// the step stage the pool is the memory of, or, for a pool that is no
    /// engine's memory, the sessions that hold it in a scope. `preempt
    /// lifo` is `By { keys: [-admission], tail: false }` (vLLM's
    /// `running[-1]`, re-queued with `prepend_request`).
    By {
        keys: Vec<CExpr>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        tail: bool,
    },
    /// A failed growth ends the lease with the least `keys` (read at
    /// `Moment::Victim`; ties to the latest admitted). The candidates are
    /// the sessions that lease the pool and hold it in no scope, in the
    /// order the pool admitted them; the victim's units go back, its
    /// `cache` applies, and the session goes on wherever it is. No hold is
    /// preempted. ThunderAgent's pause of an idle program
    /// (`docs/design/thunderagent.md`).
    ///
    /// With `every`, the pool acts at its ticks instead, at every multiple
    /// of `every` seconds: a lease's growth allocates at once, past the cap
    /// if it must, and at a tick the pool ends leases in the same order
    /// while it is over its cap (ThunderAgent's periodic `_pause_until_safe`).
    /// With `suspend` too, a lease taken is set aside, not ended: its units
    /// go back and `allocated` reads 0, and at each later tick, before it
    /// pauses, the pool gives set-aside leases back in the order of the
    /// `suspend` keys (read at `Moment::Victim`), each that fits; a
    /// `release` or the session's end drops one (ThunderAgent's resume of a
    /// paused program before its next request).
    Leases {
        keys: Vec<CExpr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        every: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        suspend: Option<Vec<CExpr>>,
    },
}

impl Preempt {
    /// The victim keys of `preempt by` or `preempt leases by`.
    pub fn keys(&self) -> Option<&[CExpr]> {
        match self {
            Preempt::None => None,
            Preempt::By { keys, .. } | Preempt::Leases { keys, .. } => Some(keys),
        }
    }

    /// The keys a `suspend by` gives set-aside leases back in.
    pub fn suspend_keys(&self) -> Option<&[CExpr]> {
        match self {
            Preempt::Leases {
                suspend: Some(keys),
                ..
            } => Some(keys),
            _ => None,
        }
    }

    /// Whether a failed growth preempts a hold, which then runs again.
    pub fn preempts_holds(&self) -> bool {
        matches!(self, Preempt::By { .. })
    }

    /// `preempt lifo`: the latest admitted, re-queued at the head.
    pub fn lifo() -> Self {
        Preempt::By {
            keys: vec![CExpr::Unary(
                UnOp::Neg,
                Box::new(CExpr::Ctx(CtxVar::Admission)),
            )],
            tail: false,
        }
    }

    /// Whether this is `preempt lifo`, the only preemption the Lean
    /// fragment knows.
    pub fn is_lifo(&self) -> bool {
        *self == Preempt::lifo()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunMode {
    /// Work in seconds at rate 1 (fifo, ps, delay).
    Plain,
    /// Step stage: prefill of `w` tokens.
    Prefill,
    /// Step stage: decode of `w` tokens, one per iteration.
    Decode,
}

/// Where an expression is evaluated. Every context variable is supplied at
/// exactly one moment (`CtxVar::moment`), and `Program::validate` rejects it
/// anywhere else: before, `age` in a session statement or `tokens` in a queue
/// key read as 0 and the program ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moment {
    /// A statement of the `init`, `turn` or `session` block, a run's work,
    /// a hold's `cache`: evaluated by the session when it gets there.
    Session,
    /// A hold's units, `reserve` and `reuse`: evaluated at admission.
    Admit,
    /// Queue keys, reevaluated for each waiting session before selection.
    Select,
    /// An eviction key or a spill clause: evaluated for one cache entry.
    Evict,
    /// A `ps` stage's capacity: evaluated for the stage's jobs.
    Ps,
    /// A step stage's `budget` and `chunk`: evaluated before the iteration,
    /// from the residents (`residents`, `decoders`, `kv_decode`, `kv_prefill`).
    Budget,
    /// A step stage's `cost`: evaluated after the iteration is scheduled,
    /// from what it scheduled (`tokens`, `prefilled`, `attention` as well).
    Step,
    /// A step stage's `serve by` keys and `serve only` predicate:
    /// evaluated for one resident.
    Serve,
    /// A pool's `preempt by` keys: evaluated for one candidate victim when
    /// a growth does not fit.
    Victim,
    /// A step stage's iteration body: an `if` guard or an `admit`'s
    /// `while`, read as the iteration is planned, from the residents and
    /// what the iteration has done so far.
    Plan,
    /// A `gauge`: evaluated on the state the deployment holds after every
    /// instant, with no session, job or resident, and held until the next
    /// one, so neither `now` nor `work(…)`, which move in between, nor
    /// `budget_left(…)`, which plans an iteration to answer.
    Gauge,
    /// A claim's `given`: read for each session once its `init` block has
    /// run (and an explicit session's preset attributes are set), from the
    /// session's attributes and the constants only: no `now`, no draw, no
    /// observable, no context variable.
    Given,
    /// A claim over the iterations of a step stage: read when an iteration
    /// starts, after its batch is scheduled (the instant its cost is read),
    /// from the cost's variables, `demand`, `served`, `now` and the pool and
    /// stage observables that do not move between events; no attribute, no
    /// draw, no `work(…)`, no `budget_left(…)`.
    Iteration,
    /// A claim `at end`: read once, when the run ends, from the constants,
    /// `now` (the end) and the aggregates of the run's observations.
    End,
    /// An engine's request at its admission: what it takes, its `reuse` and
    /// its `prefix`, read for the request the scheduler admits, from the
    /// call's arguments, the tokens it has and its cache hit.
    Request,
    /// An engine's request when it gives its pools back: its `cache`, read at
    /// a preemption (`finished` 0) and at its end (`finished` 1).
    Release,
}

impl Moment {
    /// Where an expression read at this moment is written, in a source
    /// program's words.
    pub fn place(self) -> &'static str {
        match self {
            Moment::Session => "a session statement, a run or a hold's cache",
            Moment::Admit => "a hold's header",
            Moment::Select => "a pool's queue keys",
            Moment::Evict => "an eviction key or spill clause",
            Moment::Ps => "a ps stage's capacity",
            Moment::Budget => "an engine's `tokens cap` or `each at most`",
            Moment::Step => "an engine's `worker`",
            Moment::Serve => "an engine's `advance running by (…)` keys or an `only`",
            Moment::Victim => "a pool's preempt keys",
            Moment::Plan => "an engine's `scheduler`",
            Moment::Gauge => "a gauge",
            Moment::Given => "a claim's `given`",
            Moment::Iteration => "a claim over iterations",
            Moment::End => "a claim `at end`",
            Moment::Request => "an engine's `taking`, `reuse` or `prefix`",
            Moment::Release => "an engine's `cache` or a connector's `save`",
        }
    }

    /// When it is read: what keeps the sentence true for an IR reader, who
    /// has no `tokens cap` or `scheduler` to look for.
    pub fn when(self) -> &'static str {
        match self {
            Moment::Session => "read when the session gets there",
            Moment::Admit => "read at admission",
            Moment::Select => "read before selecting a waiting session",
            Moment::Evict => "read for one cache entry",
            Moment::Ps => "read for the stage's jobs",
            Moment::Budget => "read before the iteration is formed",
            Moment::Step => "read after the iteration is formed",
            Moment::Serve => "read for each running session",
            Moment::Victim => "read for each candidate victim",
            Moment::Plan => "read as the iteration is formed",
            Moment::Gauge => "read on the deployment's state with no session",
            Moment::Given => "read on one session's attributes",
            Moment::Iteration => "read when an iteration starts",
            Moment::End => "read when the run ends",
            Moment::Request => "read when the scheduler admits the request",
            Moment::Release => "read when the request gives its pools back",
        }
    }
}

impl std::fmt::Display for Moment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}, {}", self.place(), self.when())
    }
}

/// Context variables: each exists at one `Moment` (`now` at every one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CtxVar {
    /// Simulation clock.
    Now,
    /// Queue selection: seconds since this hold entered the queue.
    Waited,
    /// Eviction keys and spill predicates: units of the entry.
    Size,
    /// Eviction keys: `now - last`.
    Age,
    /// Eviction keys: time the entry was released.
    Last,
    /// Eviction keys: 1 if the entry's session waits in a pool queue.
    Queued,
    /// PS capacity: jobs present.
    N,
    /// Step cost: tokens scheduled this iteration.
    Ntok,
    /// Step cost: decode residents scheduled.
    Ndec,
    /// Step cost: prefill tokens scheduled.
    Npre,
    /// Step cost: residents (scheduled or not).
    Nres,
    /// Step cost: memory held by the scheduled decode residents.
    Kvb,
    /// Step cost: memory held by the scheduled prefill residents.
    Kvp,
    /// Step cost: attention work of the prefill chunks, `Σ n (K + n/2)` with
    /// `K` the position before the chunk (exact for `growing` runs).
    Attn,
    /// Serve keys: 1 if the resident is decoding, 0 if prefilling.
    Decoding,
    /// Serve keys: the resident's admission sequence number (its place in
    /// vLLM's `running` list); `serve by (admission)` is admission order.
    Admission,
    /// Serve keys: tokens the resident's run has left.
    Remaining,
    /// Preempt keys: the position the candidate's hold has computed on the
    /// pool, which `computed` becomes if it is the victim.
    Position,
    /// Iteration body: the sessions this iteration has admitted so far.
    Admitted,
    /// Iteration body: the residents this iteration has preempted so far.
    Preempted,
    /// Iteration claims: the tokens the stage's residents could take in
    /// this iteration if the budget were unlimited (`min(1, remaining)` for
    /// a decode, the remaining work up to the chunk for a prefill), summed
    /// over the residents after the batch is scheduled, those `serve only`
    /// excludes included.
    Demand,
    /// Iteration claims: the tokens the stage scheduled in its earlier
    /// iterations, over the whole run.
    Served,
    /// Iteration claims: the sessions the workload has started by the
    /// iteration's start (an arrival at that instant included).
    Arrived,
    /// An engine's request: the tokens of its prompt (the call's first
    /// argument).
    Input,
    /// An engine's request: the tokens it generates (the call's second
    /// argument).
    Output,
    /// An engine's request: the tokens it has, its input and what it has
    /// generated (vLLM's `num_tokens`).
    Known,
    /// An engine's request at its admission: the cached tokens it may read,
    /// `min(cachedin(P), reuse)` on the engine's memory pool `P`.
    Hit,
    /// An engine's request at a release: 1 when it has generated its output,
    /// 0 at a preemption.
    Finished,
    /// An engine's request at its admission: the tokens its connector loads
    /// (`CConnector::load`), read after `Hit`.
    Loaded,
    /// An engine's request at its admission: 1 once it has been preempted
    /// (vLLM's `RequestStatus.PREEMPTED`, which a connector may read).
    Requeued,
}

/// How an expression's printer names a context variable (`show_expr_with`).
pub type CtxNames = fn(CtxVar) -> &'static str;

impl CtxVar {
    /// The source spelling (`link.rs` maps the same names).
    pub fn name(self) -> &'static str {
        match self {
            CtxVar::Now => "now",
            CtxVar::Waited => "waited",
            CtxVar::Size => "size",
            CtxVar::Age => "age",
            CtxVar::Last => "last",
            CtxVar::Queued => "waiting",
            CtxVar::N => "present",
            CtxVar::Ntok => "tokens",
            CtxVar::Ndec => "decoders",
            CtxVar::Npre => "prefilled",
            CtxVar::Nres => "residents",
            CtxVar::Kvb => "kv_decode",
            CtxVar::Kvp => "kv_prefill",
            CtxVar::Attn => "attention",
            CtxVar::Decoding => "decoding",
            CtxVar::Admission => "admission",
            CtxVar::Remaining => "remaining",
            CtxVar::Position => "position",
            CtxVar::Admitted => "admitted",
            CtxVar::Preempted => "preempted",
            CtxVar::Demand => "demand",
            CtxVar::Served => "served",
            CtxVar::Arrived => "arrived",
            CtxVar::Input => "request.input",
            CtxVar::Output => "request.output",
            CtxVar::Known => "request.known",
            CtxVar::Hit => "request.hit",
            CtxVar::Finished => "request.finished",
            CtxVar::Loaded => "request.loaded",
            CtxVar::Requeued => "request.preempted",
        }
    }

    /// The moments that supply the variable, as the interpreter fills its
    /// context; empty for `now`, which every moment supplies.
    pub fn moments(self) -> &'static [Moment] {
        match self {
            CtxVar::Now => &[],
            CtxVar::Waited => &[Moment::Select],
            CtxVar::Size | CtxVar::Age | CtxVar::Last | CtxVar::Queued => &[Moment::Evict],
            CtxVar::N => &[Moment::Ps],
            // the residents are known before the iteration; the tokens
            // scheduled, the prefill tokens and the attention work only after
            // (the serve keys see the residents too: they are known when the
            // order is taken)
            // (a claim over iterations reads what the cost reads)
            CtxVar::Nres | CtxVar::Ndec | CtxVar::Kvb | CtxVar::Kvp => &[
                Moment::Budget,
                Moment::Step,
                Moment::Serve,
                Moment::Plan,
                Moment::Iteration,
            ],
            // in a body, what the iteration has scheduled so far
            CtxVar::Ntok | CtxVar::Npre => &[Moment::Step, Moment::Plan, Moment::Iteration],
            CtxVar::Attn => &[Moment::Step, Moment::Iteration],
            CtxVar::Admitted | CtxVar::Preempted => &[Moment::Plan],
            CtxVar::Remaining => &[Moment::Serve],
            CtxVar::Decoding => &[Moment::Serve, Moment::Victim],
            CtxVar::Admission => &[Moment::Serve, Moment::Victim],
            CtxVar::Position => &[Moment::Victim],
            CtxVar::Demand | CtxVar::Served | CtxVar::Arrived => &[Moment::Iteration],
            CtxVar::Input | CtxVar::Output | CtxVar::Known => &[Moment::Request, Moment::Release],
            CtxVar::Hit => &[Moment::Request],
            CtxVar::Finished => &[Moment::Release],
            CtxVar::Loaded | CtxVar::Requeued => &[Moment::Request],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fun {
    Min,
    Max,
    Abs,
    Floor,
    Ceil,
    Sqrt,
    Exp,
    Ln,
    /// jobs present at a stage (queue + service)
    Queue,
    /// jobs in service at a stage
    Busy,
    /// unfinished work at a stage
    Work,
    Used,
    Free,
    /// this session's cached units in a pool
    CachedIn,
    /// this session's allocated units in a pool: its holds and its leases
    Allocated,
    Holders,
    /// sessions waiting at a pool
    Queued,
    /// online price of a miss at a stage: `price(stage, s_hit, ds)`
    Price,
    /// step stage: tokens the next iteration leaves after its residents
    BudgetLeft,
    EstLambda,
    EstRho,
    EstWait,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DistKind {
    Exp,
    Det,
    Uniform,
    Erlang,
    H2,
    Bernoulli,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CRef {
    pub base: usize,
    pub count: usize,
    pub index: Option<Box<CExpr>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CArg {
    Expr(CExpr),
    Pool(CRef),
    Stage(CRef),
}

/// The side executing a statement; requests lower to statements without
/// losing the authority under which each statement runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Workload,
    Server,
}

/// A cost belongs to a resource family. Members of an array share its units.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CostTarget {
    Joint(Vec<CostTarget>),
    Pool { base: usize, count: usize },
    Stage { base: usize, count: usize },
}

/// Request sizes are workload-owned; other scalar values are bookkeeping.
/// Cost values cannot be assigned to either kind without a type error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueType {
    Size,
    Value,
    Cost(CostTarget),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CExpr {
    /// Explicit conversion of a request quantity into this resource's cost.
    /// The conversion preserves the value and evaluation moment.
    Cost(CostTarget, Box<CExpr>),
    /// A constant. JSON has no infinity, so `inf` is written as the string
    /// `"inf"` (`-inf` as `"-inf"`) and read back from either form.
    Num(#[serde(with = "real")] f64),
    Attr(usize),
    Ctx(CtxVar),
    Sample(DistKind, Vec<CExpr>),
    Call(Fun, Vec<CArg>),
    Unary(UnOp, Box<CExpr>),
    Binary(BinOp, Box<CExpr>, Box<CExpr>),
    Cond(Box<CExpr>, Box<CExpr>, Box<CExpr>),
    /// An aggregate of every value the run observed under an observation
    /// (its index), warm-up included: read only at `Moment::End`.
    Agg(Agg, usize),
    /// A step stage's register (`Program::registers`, its index): the value
    /// its iteration body last set. Not a session's: not read in a session
    /// statement or a claim's `given`.
    Reg(usize),
}

/// The aggregates of a run's observations a claim `at end` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Agg {
    /// The sum of the values.
    Total,
    /// How many values.
    Count,
    /// The greatest value (0 when none).
    Largest,
    /// The least value (0 when none).
    Smallest,
    /// With the values sorted ascending `v1 <= … <= vn`, `Σ_k Σ_{i<=k} v_i`:
    /// the least total of the completion times of jobs of these sizes run
    /// one at a time (shortest first), whatever the order of ties.
    PrefixTotal,
}

impl Agg {
    /// The source spelling (`link.rs` maps the same names).
    pub fn name(self) -> &'static str {
        match self {
            Agg::Total => "total",
            Agg::Count => "count",
            Agg::Largest => "largest",
            Agg::Smallest => "smallest",
            Agg::PrefixTotal => "prefix_total",
        }
    }

    /// The aggregate of `values` (in any order).
    pub fn of(self, values: &[f64]) -> f64 {
        match self {
            Agg::Total => values.iter().sum(),
            Agg::Count => values.len() as f64,
            Agg::Largest => values.iter().copied().reduce(f64::max).unwrap_or(0.0),
            Agg::Smallest => values.iter().copied().reduce(f64::min).unwrap_or(0.0),
            Agg::PrefixTotal => {
                let mut v = values.to_vec();
                v.sort_by(f64::total_cmp);
                // v_i is in the prefix of every k >= i: n - i of them
                let n = v.len();
                v.iter().enumerate().map(|(i, x)| x * (n - i) as f64).sum()
            }
        }
    }
}

impl CExpr {
    /// The numerical expression inside a conversion, for readers that already
    /// know the resource (for example a stage label in a deployment figure).
    pub fn cost_value(&self) -> &Self {
        match self {
            Self::Cost(_, value) => value.cost_value(),
            _ => self,
        }
    }

    /// The first expression, this one or one in it, of which `f` holds,
    /// outermost first and then left to right; the index of a pool or
    /// stage reference is in it (#273).
    pub fn find(&self, f: &impl Fn(&CExpr) -> bool) -> Option<&CExpr> {
        if f(self) {
            return Some(self);
        }
        match self {
            CExpr::Num(_) | CExpr::Attr(_) | CExpr::Ctx(_) | CExpr::Agg(..) | CExpr::Reg(_) => None,
            CExpr::Sample(_, xs) => xs.iter().find_map(|x| x.find(f)),
            CExpr::Call(_, args) => args.iter().find_map(|a| match a {
                CArg::Expr(x) => x.find(f),
                CArg::Pool(r) | CArg::Stage(r) => r.index.as_ref().and_then(|i| i.find(f)),
            }),
            CExpr::Unary(_, x) | CExpr::Cost(_, x) => x.find(f),
            CExpr::Binary(_, a, b) => a.find(f).or_else(|| b.find(f)),
            CExpr::Cond(c, a, b) => c.find(f).or_else(|| a.find(f)).or_else(|| b.find(f)),
        }
    }

    /// Whether `f` holds of this expression or of one in it.
    pub fn any(&self, f: &impl Fn(&CExpr) -> bool) -> bool {
        self.find(f).is_some()
    }
}

/// An `f64` in JSON, infinities included: a number when finite, the string
/// `"inf"` or `"-inf"` otherwise (JSON has no infinity, and serde_json
/// writes `null`, which does not read back). Used for `CExpr::Num` and a
/// pool's `cap`.
mod real {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(untagged)]
    enum Real {
        Num(f64),
        Str(String),
    }

    pub fn serialize<S: Serializer>(x: &f64, s: S) -> Result<S::Ok, S::Error> {
        if x.is_finite() {
            Real::Num(*x).serialize(s)
        } else {
            Real::Str(if *x < 0.0 { "-inf" } else { "inf" }.into()).serialize(s)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        match Real::deserialize(d)? {
            Real::Num(x) => Ok(x),
            Real::Str(s) => match s.as_str() {
                "inf" => Ok(f64::INFINITY),
                "-inf" => Ok(f64::NEG_INFINITY),
                other => Err(serde::de::Error::custom(format!(
                    "expected a number, \"inf\" or \"-inf\", found \"{other}\""
                ))),
            },
        }
    }
}

pub type BlockId = usize;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CStmt {
    Turn,
    Set(usize, CExpr),
    Observe(usize, CExpr),
    Hold {
        pools: Vec<(CRef, CExpr, Option<CExpr>)>,
        reuse: Option<CExpr>,
        body: BlockId,
        cache: Option<CExpr>,
        /// `prefix (k₁: n₁, …)`: what the hold's sequence is made of, as a
        /// chain of segments, each a key and a length. Two holds share the
        /// first `j` segments of their pool's prefix cache when their first
        /// `j` keys are equal. `None` is the chain `(serial: ∞)`: the
        /// session's own prefix, shared with no other session.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefix: Option<Vec<(CExpr, CExpr)>>,
        /// The pool (one of `pools`) whose allocation outlives the scope as
        /// a lease, and for how long at most: it stays allocated, neither
        /// evictable nor a preemption victim, until the session's `Release`
        /// of it, the expiry, or the session's end; then `cache` applies.
        /// vLLM's prefiller keeps a finished request's blocks this way for
        /// the decoder's read (`delay_free_blocks`).
        lease: Option<(CRef, CExpr)>,
    },
    Grow(CRef, CExpr),
    /// Give back `e` units of the innermost hold's allocation on the pool,
    /// or of the session's lease of it outside every hold: the inverse of
    /// `Grow`. A hold keeps at least what it has computed. ThunderAgent
    /// counts a program calling a tool at a weight below 1 this way.
    Shrink(CRef, CExpr),
    Drop(CRef),
    /// Release the innermost enclosing hold's allocation on the pool now,
    /// caching per that hold's `cache` clause; the scope's end then has
    /// nothing left there. A pool the session holds nothing of is a no-op (a
    /// hold re-executed after a preemption reaches the statement again). The
    /// KV of a prefill/decode split lives in two pools whose lifetimes
    /// overlap without nesting - the decode instance allocates before the
    /// prefill instance frees - which a scope alone cannot say.
    Release(CRef),
    /// The KV of `e` tokens arrives from outside the engine (a NIXL read, an
    /// offload tier): the innermost enclosing hold's computed position on the
    /// pool advances by `e`, within its allocation (a program that needs
    /// more grows first). What a `growing` run does token by token, at once.
    Load(CRef, CExpr),
    Run {
        stage: CRef,
        mode: RunMode,
        work: CExpr,
        growing: Option<CRef>,
        /// Further stages the same job holds from its start to its end: a
        /// flow over `stage` and these, its rate set by `share` from their
        /// capacities. Empty for a run on one stage.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        also: Vec<CRef>,
        /// How the flows on this run's stages divide them: present exactly
        /// when `also` is not empty, and the same for every run whose stages
        /// some run joins to these (`Program::stage_shares`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        share: Option<Share>,
        /// A fixed wait before the work starts, in seconds: the job is at
        /// its stages from the run's start and takes none of their capacity
        /// until the wait ends (posting a NIXL READ and the step that polls
        /// its completion: `kvt.nixl_pull`'s `latency`, a link's `serve …
        /// latency`, a run's `(w, latency = x)`). The stage report counts it
        /// as the run's wait; a run of zero work with one completes when it
        /// ends. Read at the run's start, like `work`; absent: no wait.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        latency: Option<CExpr>,
        /// Which of the run's stages (`stage` is 0, `also[k]` is `k + 1`)
        /// posts the copy, for a KV relation's copy: the reader's NIC in a
        /// NIXL pull, the source's in a push (`kvt.nixl_pull`,
        /// `kvt.nixl_push`). It says who starts the copy, which the figure
        /// draws; the run is the same either way. Absent: no side is named
        /// (a `transfer on` written by hand).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        poster: Option<usize>,
    },
    Branch(CExpr, BlockId, BlockId),
    Loop(BlockId),
    /// Test the guard before each pass; continue after the loop when it is zero.
    While(CExpr, BlockId),
    Choose {
        var: usize,
        count: CExpr,
        key: Vec<CExpr>,
    },
    End,
    /// Run the block beside the session, as a leg of the same request: it
    /// starts now, with a copy of the session's attributes, at the same
    /// time as the statements after the fork. vLLM's push proxy sends the
    /// prefill and the decode request of one request at once this way. A
    /// leg's holds are its own and its `set`s change only its copy, which
    /// ends with it; the leases it leaves pass to the session when it ends.
    /// A leg may not `turn`, `end`, fork or join.
    Fork(BlockId),
    /// Wait until every leg the session has forked has ended.
    Join,
    /// A request to an engine: `input` tokens of prompt, `output` tokens to
    /// generate. The engine keeps its state (the tokens it has, its KV
    /// position, what it has generated), admits it by its `request`, runs its
    /// prefill and its decode, and the session goes on when it has generated
    /// `output` (`docs/design/scheduler-worker.md`, step 3).
    Request {
        stage: CRef,
        input: CExpr,
        output: CExpr,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CSpill {
    pub to: usize,
    pub via: usize,
    pub work: CExpr,
    pub when: CExpr,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CPool {
    pub name: String,
    /// The member's index in an array (`pool kv[N]`), `None` for a single
    /// pool: a label for the report, which the simulation does not read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    /// Capacity in units; `inf` (a pool without `cap`) is the string
    /// `"inf"` in JSON, as for `CExpr::Num`.
    #[serde(with = "real")]
    pub cap: f64,
    pub block: Option<f64>,
    pub evict: CEvict,
    pub preempt: Preempt,
    /// Ascending lexicographic keys, reevaluated before every selection;
    /// None is FIFO. Equal keys retain queue order; resumed holds go first.
    pub queue: Option<Vec<CExpr>>,
    /// `queue … every (dt)`: the pool admits waiting holds only at every
    /// multiple of `dt` seconds, in the queue's order, each admitted one
    /// running until it blocks before the next is tried, and one that does
    /// not fit passed over; a hold queued during a tick waits for the next.
    /// The pools' ticks at one instant act after its other events, in the
    /// pools' order, a pool's queue tick before its preemption tick
    /// (ThunderAgent's global waiting queue, drained at the router's tick).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_every: Option<f64>,
    pub spill: Option<CSpill>,
    pub admit_via: Option<usize>,
    /// `reserve held`: what a hold's `reserve` tested and it has not
    /// allocated counts against every later admission while the hold lasts
    /// (TensorRT-LLM's `GUARANTEED_NO_EVICT`), and the holder grows into
    /// it. Without it a `reserve` is a test at admission alone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reserve_held: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CEvict {
    Lru,
    By(Vec<CExpr>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CStep {
    pub budget: CExpr,
    pub cost: CExpr,
    pub chunk: CExpr,
    /// `granule g`: a prefill gets the whole of what it has left, or a
    /// multiple of `g` (rounded down; none when that is 0). `inf` schedules
    /// a prefill whole or not at all (TensorRT-LLM without chunking), a
    /// block size aligns its chunks. None is any amount (as `1` is, for
    /// whole tokens). A prefill it refuses ends the iteration's admissions.
    /// A constant above 0, `inf` included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granule: Option<CExpr>,
    /// How the iteration serves its residents: an order, or the
    /// exclusive-prefill rule.
    pub serve: CServe,
    pub memory: Option<usize>,
    /// The iteration as the program writes it (`iteration { … }`): which
    /// residents are served, in what order, and when the waiting are
    /// admitted, in the order the statements run. None is vLLM's procedure,
    /// `serve` then the waiting admitted while the iteration has not
    /// preempted, in the order above. Not with `ExclusivePrefill`, a rule a
    /// body cannot write (it takes back decodes already chosen). A stage's
    /// `serve only (p)` is the body `[Serve {only: p}, Admit {only: p, gate:
    /// !preempted}]`, which the linker writes (#355).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration: Option<Vec<CIter>>,
    /// What the engine's admission takes of a request and what its release
    /// leaves cached (`CStmt::Request`). `None`: the engine takes nothing
    /// and caches nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<Box<CRequest>>,
    /// The engine's KV connector (`connector C { … }`): what its requests
    /// load from an L2 at admission and save to it at their end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector: Option<Box<CConnector>>,
}

/// An engine's KV connector to an L2 queue (vLLM's `KVConnector`, LMCache's
/// in `lmcache_mp_connector.py`): the tokens a request loads from the L2 at
/// its admission, read after `request.hit` (`get_num_new_matched_tokens`),
/// and the tokens the L2 holds of it after it ends (`request_finished`,
/// which keeps its blocks until the save is done). Each is the kernel block
/// it means, which the session runs while the request holds its pools: a
/// load `run C (cost(C, E.loaded · r)); load P (cost(P, E.loaded));` into
/// the request's hold on the engine's memory `P`, a save the hold on the
/// L2's pool that copies what the L2's entry lacks. The interpreter sets
/// the attributes `loaded` and `saved` (`E.loaded`, `E.saved`) before it
/// runs them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CConnector {
    /// The tokens to load (`Moment::Request`), and the block that loads them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load: Option<(CExpr, BlockId)>,
    /// The tokens the L2 is to hold (`Moment::Release`, with
    /// `request.finished` 1), and the block that saves them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save: Option<(CExpr, BlockId)>,
    /// Attribute slots: what the latest admission loaded, what the save
    /// copies to.
    pub loaded: usize,
    pub saved: usize,
    /// Attribute slot: when its latest load ended (`E.load_done`).
    pub load_done: usize,
    /// A KV relation's (`kvt.nixl_pull`): the attribute its request's entry
    /// sets to the source's member it took `from`, -1 for none, which loads
    /// nothing (`E.from`, vLLM's `do_remote_prefill`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<usize>,
    /// The observation the engine records for each admission, `E.loaded`.
    pub record: usize,
}

impl CConnector {
    /// Its blocks, a load's then a save's.
    pub fn blocks(&self) -> impl Iterator<Item = BlockId> + '_ {
        self.load.iter().chain(&self.save).map(|(_, b)| *b)
    }
}

/// How an engine admits and releases a request: a hold's header, read for
/// the request (`Moment::Request`, `Moment::Release`). The memory pool
/// (`CStep::memory`), when taken, grows with the request's KV position and
/// gives `request.hit`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CRequest {
    /// Each pool taken, its units and its `reserve`.
    pub pools: Vec<(CRef, CExpr, Option<CExpr>)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reuse: Option<CExpr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CExpr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<Vec<(CExpr, CExpr)>>,
    /// The observations the engine records of its requests.
    pub records: RequestRecords,
}

/// What an engine records of its requests. Every record is an attribute of
/// the session, which its caller reads after the call: `E.reused` and
/// `E.prefill` (the cached tokens its latest admission read and the tokens
/// its prefill computes), `E.admitted` (when it was last admitted) and
/// `E.first` (when its first token was sampled). What each admission
/// records is an observation of the report too (`reused`, `prefill`, slots
/// of `Program::observes`, named alike).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RequestRecords {
    pub reused: usize,
    pub prefill: usize,
    pub admitted: usize,
    pub first: usize,
    /// The attributes of `reused` and `prefill`.
    pub last_reused: usize,
    pub last_prefill: usize,
}

/// A statement of a step stage's iteration body. Each runs once where it is
/// written; the body has no loop, so an iteration ends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CIter {
    /// Serve the residents this iteration has not served yet, in `by`'s
    /// order (the stage's `serve` order when None), skipping those `only`
    /// reads as 0, which a later `Serve` may serve, while budget is left.
    /// Both are read at `Moment::Serve`.
    Serve {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        only: Option<CExpr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<Vec<CExpr>>,
    },
    /// Admit the head of the queues the stage serves (`admit_via`) and serve
    /// each newcomer that `only` (read at `Moment::Serve`) does not read as
    /// 0, one at a time, while budget is left, the head fits and `gate`
    /// (read at `Moment::Plan` before each) is 1. A newcomer `only`
    /// excludes is admitted and left unserved, as the stage's `serve only`
    /// leaves it: `serve only (p)` on the stage is the body `serve only (p);
    /// admit only (p) while (!preempted);`.
    Admit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        only: Option<CExpr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gate: Option<CExpr>,
    },
    /// A test read at `Moment::Plan`: the first body when 1, the second
    /// when 0.
    Branch(CExpr, Vec<CIter>, Vec<CIter>),
    /// Set a register of this stage (`Program::registers`) to an expression
    /// read at `Moment::Plan`. It takes effect with the iteration: an
    /// iteration that schedules nothing and preempts nothing is none, and
    /// its sets are undone.
    Set(usize, CExpr),
}

/// A step stage's serving policy: resident order (`By(keys)`) or the rule
/// that a prefill runs alone
/// (`ExclusivePrefill`, which selects one prefill alone, including a waiting
/// prefill that displaces tentative resident decodes; it is not an order,
/// and one field means a program cannot combine it with another order).
/// Two booleans described this before
/// (`exclusive_prefill`, `decode_first`) and could both be set; the Lean
/// fragment reads only `By([])`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CServe {
    /// Ascending keys, evaluated per resident at `Moment::Serve`
    /// (`Decoding`, `Admission`, `Remaining`, the residents' `Nres`, `Ndec`,
    /// `Kvb`, `Kvp`, and `Now`), ties in admission order. No keys is
    /// admission order itself (vLLM's `running` list; `serve admission`), and
    /// `decode first` is `By([decoding ? 0 : 1])`. A key may not draw: it is
    /// read for every resident at every iteration.
    By(Vec<CExpr>),
    /// A prefill runs alone; resident prefills take precedence. With only
    /// decodes selected, a waiting prefill that fits replaces them and gets
    /// the full token budget. No further waiting admission follows a selected
    /// prefill. Only final selections advance computed KV; allocations for
    /// displaced decodes remain held. Ordinary capacity/budget/preemption
    /// gates still apply; this is not a PP/remote-KV scheduler preset.
    ExclusivePrefill,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CStageKind {
    Fifo(usize),
    Ps(CExpr),
    Step(CStep),
}

impl CStageKind {
    /// `delay`: `ps(present)`, every job at rate 1 with no waiting, an
    /// infinite server. The frontend's `delay` lowers to it; the interpreter
    /// and the drawing recognise it (13).
    pub fn delay() -> Self {
        CStageKind::Ps(CExpr::Ctx(CtxVar::N))
    }

    pub fn is_delay(&self) -> bool {
        matches!(self, CStageKind::Ps(CExpr::Ctx(CtxVar::N)))
    }

    /// The kind as a program writes it, for a message: `fifo`, `delay`,
    /// `ps`, `step`.
    pub fn word(&self) -> &'static str {
        match self {
            CStageKind::Fifo(_) => "fifo",
            k if k.is_delay() => "delay",
            CStageKind::Ps(_) => "ps",
            CStageKind::Step(_) => "step",
        }
    }

    /// What a message calls the stage, as a program declares it: `engine`
    /// for a step stage, `stage` for any other.
    pub fn noun(&self) -> &'static str {
        match self {
            CStageKind::Step(_) => "engine",
            _ => "stage",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CStage {
    pub name: String,
    /// The member's index in an array (`stage E[N]`), `None` for a single
    /// stage: a label for the report, which the simulation does not read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    pub kind: CStageKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CArrival {
    Poisson(f64),
    /// Open renewal process with a constant or sampled interarrival-time expression.
    Renewal(CExpr),
    Closed(usize),
    Batch(usize),
    /// Explicit sessions, all arriving at time 0: each one's attributes
    /// are preset after its `init` block runs. The workload instance of a
    /// scenario (e.g. the vLLM oracle's requests) as data rather than as
    /// program text.
    Sessions(Vec<SessionInit>),
    None,
}

/// A serQ program in IR form: the deployment (pools, stages), the workload
/// (arrival, `init`/`turn` blocks, trace, or explicit sessions), the session
/// (statement blocks) and the run parameters. Every name is resolved to an
/// index and every constant is folded; the tables `attrs` and `observes`
/// keep the names for tools.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Program {
    /// IR format version (`IR_VERSION`).
    pub version: u32,
    pub attrs: Vec<String>,
    pub attr_types: Vec<ValueType>,
    /// Mandatory, aligned with `blocks`, including every nested statement.
    pub sides: Vec<Vec<Side>>,
    pub observes: Vec<String>,
    pub pools: Vec<CPool>,
    pub stages: Vec<CStage>,
    pub arrival: CArrival,
    pub trace: Option<String>,
    pub trace_ordered: bool,
    pub init: BlockId,
    pub turn: BlockId,
    pub session: BlockId,
    pub blocks: Vec<Vec<CStmt>>,
    pub horizon: f64,
    pub warmup: f64,
    pub seed: u64,
    /// Maximum open-population arrivals before draining active sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrivals: Option<usize>,
    /// Slots of the built-in attributes.
    pub slot_cached: usize,
    pub slot_serial: usize,
    pub slot_turn: usize,
    pub slot_new: usize,
    pub slot_out: usize,
    pub slot_think: usize,
    pub slot_more: usize,
    pub slot_forced: usize,
    /// `computed`: the position the session's hold had computed when it was
    /// preempted (what its `growing` runs had computed), 0 when it enters a
    /// hold for the first time or after a hold completed. A re-executed hold
    /// reads it to resume: vLLM keeps a preempted request's generated tokens
    /// and recomputes their KV (`_preempt_request` resets
    /// `num_computed_tokens` only, scheduler.py:1560-1561).
    pub slot_computed: usize,
    /// Attribute slots the scheduler may not read: legal at `Moment::Session`
    /// only, rejected by `validate` at every other moment (a hold's header, a
    /// queue or eviction key, a spill clause, a ps capacity, a step stage's
    /// budget, cost, chunk or serve keys); what the scheduler itself sets
    /// (`cached`, `computed`) cannot be hidden from it. The output length
    /// `o` is the case: vLLM knows `max_tokens` (scheduler.py:639) and learns
    /// the length when `check_stop` sees EOS or the cap (sched/utils.py:98-119,
    /// called at scheduler.py:2426), so a program that reserves `prompt + o`
    /// is one vLLM cannot be.
    pub hidden: Vec<usize>,
    /// `gauge NAME = e;`: functions of the deployment's state whose time
    /// average the report gives. They read and do not act, so a reader that
    /// ignores them runs the same program (`docs/ir.md`, Stability).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gauges: Vec<Gauge>,
    /// `claim NAME …;`: propositions about every path of the program, which
    /// the interpreter checks on the path it runs and the report states.
    /// They read and do not act, so a reader that ignores them runs the
    /// same program (`docs/ir.md`, Stability).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claims: Vec<Claim>,
    /// `state NAME = c;` of the step stages: registers a stage's iteration
    /// body sets and the scheduler's expressions read (`CExpr::Reg`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub registers: Vec<Register>,
}

/// A step stage's register: its name, the stage whose body sets it, and its
/// value before the first iteration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Register {
    pub name: String,
    pub stage: usize,
    #[serde(with = "real")]
    pub init: f64,
}

/// A gauge: a name and an expression evaluated at `Moment::Gauge`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gauge {
    pub name: String,
    pub expr: CExpr,
}

/// A claim: `expr` holds of every iteration of a step stage, of some
/// iteration of it, or at the end of the run (`kind`), on every path whose
/// sessions all satisfy `given`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub name: String,
    /// Read at `Moment::Given` for each session: a session for which it is
    /// 0 puts the path out of the claim's scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub given: Option<CExpr>,
    pub kind: ClaimKind,
    /// Read at `Moment::Iteration` for an iteration claim, at `Moment::End`
    /// for one `at end`.
    pub expr: CExpr,
}

/// What a claim quantifies over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClaimKind {
    /// Every iteration of the step stage (its index) satisfies the claim.
    EveryIteration(usize),
    /// Some iteration of the step stage (its index) satisfies the claim.
    SomeIteration(usize),
    /// The run's end satisfies the claim.
    AtEnd,
}

impl ClaimKind {
    /// The moment the claim's expression is read at.
    pub fn moment(self) -> Moment {
        match self {
            ClaimKind::EveryIteration(_) | ClaimKind::SomeIteration(_) => Moment::Iteration,
            ClaimKind::AtEnd => Moment::End,
        }
    }

    /// The stage an iteration claim is about.
    pub fn stage(self) -> Option<usize> {
        match self {
            ClaimKind::EveryIteration(s) | ClaimKind::SomeIteration(s) => Some(s),
            ClaimKind::AtEnd => None,
        }
    }
}

/// The sharing policy of a run's `share`: how flows that hold several `ps`
/// stages at once divide their capacities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Share {
    /// Max-min fair: every flow's rate rises together until a stage fills,
    /// the flows through it stop there, and the rest go on.
    MaxMin,
    /// Each flow gets its equal share at the tightest of its stages,
    /// `min over s of φ_s / n_s`; what that leaves at the others is unused.
    Bottleneck,
}

impl Share {
    /// The policies std/share supplies, by the name a program reads each
    /// as (`share.maxmin`). Every site that names one reads this table.
    pub const ALL: [(&'static str, Share); 2] =
        [("maxmin", Share::MaxMin), ("bottleneck", Share::Bottleneck)];

    /// The policy's name in std/share.
    pub fn name(self) -> &'static str {
        Share::ALL
            .iter()
            .find(|(_, s)| *s == self)
            .map(|(n, _)| *n)
            .expect("every policy is in the table")
    }
}

/// One explicit session of `CArrival::Sessions`: preset attributes
/// (slot, value) and, optionally, the session's turns. A session with turns
/// draws them in order at every `turn` statement instead of from the trace
/// corpus: each sets its (slot, value) pairs, `more` is 1 while another turn
/// remains (0 after the last), and `turn_no` counts turns, as for an
/// ordered trace.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct SessionInit {
    pub attrs: Vec<(usize, f64)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<Vec<(usize, f64)>>,
}

impl Program {
    /// The policy of each stage some run over several stages holds, each
    /// array whole (a run's index is known only when it starts), and `None`
    /// for every other stage. The stages runs join, however indirectly, are
    /// one fabric: their flows are solved together, so a fabric has one
    /// policy, and one whose runs name two is an error, as is a run over
    /// several stages that names none; the error is about the run that
    /// names none, or the first that names a policy its fabric's runs did
    /// not. Every job on a stage with a policy is a flow of it; every other
    /// stage serves as its kind says.
    pub fn stage_shares(&self) -> Result<Vec<Option<Share>>, Invalid> {
        let n = self.stages.len();
        let mut up: Vec<usize> = (0..n).collect();
        fn root(up: &mut [usize], mut k: usize) -> usize {
            while up[k] != k {
                up[k] = up[up[k]];
                k = up[k];
            }
            k
        }
        let mut runs = vec![];
        for (bi, b) in self.blocks.iter().enumerate() {
            for (si, st) in b.iter().enumerate() {
                if let CStmt::Run {
                    stage, also, share, ..
                } = st
                    && !also.is_empty()
                {
                    let ks: Vec<usize> = std::iter::once(stage)
                        .chain(also)
                        .flat_map(|r| r.base..(r.base + r.count.max(1)).min(n))
                        .collect();
                    for w in ks.windows(2) {
                        let (a, b) = (root(&mut up, w[0]), root(&mut up, w[1]));
                        up[a] = b;
                    }
                    if let Some(&k) = ks.first() {
                        runs.push((k, *share, bi, si));
                    }
                }
            }
        }
        let mut policy: Vec<Option<Share>> = vec![None; n];
        for (k, share, bi, si) in runs {
            let Some(share) = share else {
                return Err(Invalid::at(
                    format!(
                        "the run over several stages from stage `{}` names no `share`: \
                         `share.maxmin` or `share.bottleneck`",
                        self.stages[k].name
                    ),
                    bi,
                    si,
                ));
            };
            let r = root(&mut up, k);
            match policy[r] {
                Some(p) if p != share => {
                    let mut joined: Vec<&str> = vec![];
                    for j in 0..n {
                        let name = self.stages[j].name.as_str();
                        if root(&mut up, j) == r && !joined.contains(&name) {
                            joined.push(name);
                        }
                    }
                    return Err(Invalid::at(
                        format!(
                            "this run names `share.{}`, and an earlier run on its fabric \
                             `share.{}`: runs join the stages `{}`, so their flows are solved \
                             together, and one fabric has one policy",
                            share.name(),
                            p.name(),
                            joined.join("`, `"),
                        ),
                        bi,
                        si,
                    ));
                }
                _ => policy[r] = Some(share),
            }
        }
        Ok((0..n).map(|k| policy[root(&mut up, k)]).collect())
    }

    /// Every block the program reaches from `init`, `turn` and `session` is
    /// reached once: the blocks form a tree, as the linker builds them, so
    /// the walks over them (`enclosed`, `lets_time_pass`) end. IR from JSON
    /// could otherwise point a body at its own ancestor (#272).
    fn blocks_are_a_tree(&self) -> Result<(), String> {
        let mut seen = vec![false; self.blocks.len()];
        let mut todo = vec![self.init, self.turn, self.session];
        // an engine's connector runs its blocks inside a request
        todo.extend(self.connectors().flat_map(|(_, c)| c.blocks()));
        while let Some(b) = todo.pop() {
            let Some(was) = seen.get_mut(b) else {
                return Err(format!("block {b} out of range"));
            };
            if *was {
                return Err(format!(
                    "block {b} is reached twice: a program's blocks form a tree, each the body \
                     of one statement"
                ));
            }
            *was = true;
            for st in &self.blocks[b] {
                match st {
                    CStmt::Hold { body, .. }
                    | CStmt::Loop(body)
                    | CStmt::While(_, body)
                    | CStmt::Fork(body) => todo.push(*body),
                    CStmt::Branch(_, a, c) => todo.extend([*a, *c]),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// `grow`, `load`, `release` and a `growing` run act on the innermost
    /// hold of their pool around them, as the reference is written, index
    /// included; `release` may instead end a lease of it; a hold leases
    /// one of its own pools (#272). Holds nest as the blocks do.
    ///
    /// A hold's index is read at admission, again at every statement that
    /// acts on the hold, and again when a preempted hold is admitted anew;
    /// the readings must name one member (#282). So a hold's body may not
    /// change an attribute its index reads (`set`, `choose`, `turn`, or a
    /// hold inside it setting `cached` and `computed`), and an index read at
    /// a statement inside reads only attributes and numbers, not the state
    /// or the clock, which move between the readings.
    fn enclosed<'a>(
        &'a self,
        b: BlockId,
        held: &mut Vec<(&'a CRef, Option<&'static str>)>,
        leased: &[&'a CRef],
    ) -> Result<(), Invalid> {
        let need =
            |held: &[(&CRef, Option<&'static str>)], r: &CRef, what: &str, or_leased: bool| {
                let name = &self.pools[r.base].name;
                let shown = match &r.index {
                    None => name.clone(),
                    Some(_) => format!("{name}[…]"),
                };
                if let Some(&(h, moves)) = held.iter().rev().find(|(h, _)| *h == r) {
                    return match moves {
                        None => Ok(()),
                        Some(read) => Err(format!(
                            "`{what} {name}[{}]`: the hold's index reads {read}, which moves \
                         between admission and this statement, so the two may name different \
                         members; name the member in an attribute",
                            h.index
                                .as_ref()
                                .map_or(String::new(), |i| self.show_expr(i))
                        )),
                    };
                }
                let leased: &[&CRef] = if or_leased { leased } else { &[] };
                if leased.contains(&r) {
                    return Ok(());
                }
                let hint = if held
                    .iter()
                    .map(|(h, _)| h)
                    .chain(leased)
                    .any(|h| h.base == r.base)
                {
                    ": write the pool as the hold does, index included"
                } else if or_leased {
                    ": it acts on an enclosing hold's allocation, or on the session's lease of it"
                } else {
                    ": it acts on an enclosing hold's allocation"
                };
                Err(format!(
                    "`{what} {shown}` outside a hold of `{shown}`{hint}"
                ))
            };
        for (k, s) in self.blocks[b].iter().enumerate() {
            let here = |m: String| Invalid::at(m, b, k);
            match s {
                CStmt::Hold {
                    pools, body, lease, ..
                } => {
                    if let Some((r, _)) = lease
                        && !pools.iter().any(|(q, _, _)| q == r)
                    {
                        return Err(here(format!(
                            "`lease {}`: the hold does not take that pool (write it as the hold \
                             does, index included)",
                            self.pools[r.base].name
                        )));
                    }
                    let depth = held.len();
                    // a hold that a pool of its own or of a hold around it may
                    // preempt runs again, admitted anew: its indices are read
                    // again, after admission set `cached` and the preemption
                    // `computed` (#317)
                    let preempter = held
                        .iter()
                        .map(|(h, _)| *h)
                        .chain(pools.iter().map(|(r, _, _)| r))
                        .flat_map(|r| &self.pools[r.base..r.base + r.count])
                        .find(|q| q.preempt.preempts_holds());
                    if let Some(q) = preempter
                        && let Some((r, _, _)) = pools.iter().find(|(r, _, _)| {
                            r.index.as_ref().is_some_and(|i| {
                                moves(i)
                                    || i.any(&|x| {
                                        matches!(x, CExpr::Attr(a)
                                            if *a == self.slot_cached || *a == self.slot_computed)
                                    })
                            })
                        })
                    {
                        return Err(here(format!(
                            "`hold {}`: `{}` may preempt it, and it is admitted anew and reads \
                             the index again, which reads the state, the clock, or the `cached` \
                             and `computed` that admission and preemption set; name the member \
                             in another attribute",
                            self.show_pool_ref(r),
                            q.name
                        )));
                    }
                    let set = self.assigned(*body);
                    for (r, _, _) in pools {
                        let Some(i) = &r.index else { continue };
                        if let Some(&a) = set
                            .iter()
                            .find(|&&a| i.any(&|x| matches!(x, CExpr::Attr(s) if *s == a)))
                        {
                            return Err(here(format!(
                                "`hold {}[{}]`: its body changes `{}`, which the index reads, so \
                                 the member read at admission, at a statement inside and after a \
                                 preemption may differ; set it before the hold",
                                self.pools[r.base].name,
                                self.show_expr(i),
                                self.attrs.get(a).map_or("?", |s| s.as_str())
                            )));
                        }
                    }
                    held.extend(pools.iter().map(|(r, _, _)| {
                        let moving = r
                            .index
                            .as_ref()
                            .and_then(|i| moves(i).then_some("the state or the clock"));
                        (r, moving)
                    }));
                    self.enclosed(*body, held, leased)?;
                    held.truncate(depth);
                }
                CStmt::Grow(r, _) => need(held, r, "grow", true).map_err(here)?,
                CStmt::Shrink(r, _) => need(held, r, "shrink", true).map_err(here)?,
                CStmt::Load(r, _) => need(held, r, "load", false).map_err(here)?,
                CStmt::Release(r) => need(held, r, "release", true).map_err(here)?,
                CStmt::Run {
                    growing: Some(g), ..
                } => need(held, g, "growing", false).map_err(here)?,
                CStmt::Branch(_, a, c) => {
                    self.enclosed(*a, held, leased)?;
                    self.enclosed(*c, held, leased)?;
                }
                CStmt::Loop(x) | CStmt::While(_, x) => self.enclosed(*x, held, leased)?,
                CStmt::Fork(x) => {
                    // a preempted hold runs again from its start, and would
                    // fork a second leg; the proxy sends each leg once
                    if let Some(q) = held
                        .iter()
                        .flat_map(|(r, _)| &self.pools[r.base..r.base + r.count])
                        .find(|q| q.preempt.preempts_holds())
                    {
                        return Err(here(format!(
                            "`fork` inside a hold of `{}`, which may preempt it: the hold runs \
                             again from its start and forks a second leg; fork before the hold",
                            q.name
                        )));
                    }
                    // a leg holds nothing of the session's: its holds are its own
                    self.enclosed(*x, &mut vec![], leased)?
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The attributes a block may change, in it or in a block in it: what it
    /// sets or chooses, every attribute at a `turn` (the workload's `turn`
    /// block and the scheduler's marks), and `cached` and `computed` at a hold.
    fn assigned(&self, b: BlockId) -> Vec<usize> {
        let mut out = vec![];
        for s in &self.blocks[b] {
            match s {
                CStmt::Set(a, _) | CStmt::Choose { var: a, .. } => out.push(*a),
                CStmt::Turn => out.extend(0..self.attrs.len()),
                CStmt::Hold { body, .. } => {
                    out.extend([self.slot_cached, self.slot_computed]);
                    out.extend(self.assigned(*body));
                }
                CStmt::Loop(body) | CStmt::While(_, body) => out.extend(self.assigned(*body)),
                CStmt::Branch(_, x, y) => {
                    out.extend(self.assigned(*x));
                    out.extend(self.assigned(*y));
                }
                _ => {}
            }
        }
        out
    }

    /// A run over several stages holds `ps` stages of a constant capacity
    /// (a flow is not described by the `n` a capacity could read), each
    /// stage array once (an index is known only when the run starts), and
    /// names its `share`, one per fabric (`stage_shares`); a run on one
    /// stage names none, and every run on a shared stage is plain.
    fn validate_flows(&self) -> Result<(), String> {
        // `validate_located` checked the fabrics, with their statements
        let shared: Vec<bool> = self
            .stage_shares()
            .map_err(|e| e.message)?
            .iter()
            .map(Option::is_some)
            .collect();
        for (k, on) in shared.iter().enumerate() {
            if *on && !matches!(self.stages[k].kind, CStageKind::Ps(CExpr::Num(c)) if c > 0.0) {
                return Err(format!(
                    "stage `{}` is held with another stage by one run, so its capacity is shared: it must be `ps(φ)` with a constant φ above 0",
                    self.stages[k].name
                ));
            }
        }
        for b in &self.blocks {
            for st in b {
                let CStmt::Run {
                    stage,
                    mode,
                    growing,
                    also,
                    share,
                    poster,
                    ..
                } = st
                else {
                    continue;
                };
                if poster.is_some_and(|k| k > also.len()) || (poster.is_some() && also.is_empty()) {
                    return Err(format!(
                        "a run on `{}` names its poster as stage {} of {}: a poster is one of \
                         the stages of a copy over several",
                        self.stages[stage.base].name,
                        poster.unwrap_or(0),
                        1 + also.len()
                    ));
                }
                if also.is_empty() && share.is_some() {
                    return Err(format!(
                        "a run on the one stage `{}` names a `share`: a policy divides the stages \
                         a run holds at once, and a run on a shared stage takes its fabric's",
                        self.stages[stage.base].name
                    ));
                }
                let on_shared = (stage.base..stage.base + stage.count.max(1))
                    .any(|k| shared.get(k).copied().unwrap_or(false));
                if on_shared && (*mode != RunMode::Plain || growing.is_some()) {
                    return Err(format!(
                        "a run on the shared stage `{}` is plain: no `prefill`/`decode`, no `growing`",
                        self.stages[stage.base].name
                    ));
                }
                let mut bases = vec![stage.base];
                for r in also {
                    if bases.contains(&r.base) {
                        return Err(format!(
                            "stage `{}` is named twice in one run",
                            self.stages[r.base].name
                        ));
                    }
                    bases.push(r.base);
                }
            }
        }
        Ok(())
    }

    /// JSON form of the IR.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("IR serialises")
    }

    /// Read and validate an IR from JSON.
    pub fn from_json(s: &str) -> Result<Program, String> {
        // the version first: a field can keep its name across versions and
        // change its type (`Choose.key` in 7), and the shape's error would
        // hide the version's
        #[derive(Deserialize)]
        struct Version {
            version: u32,
        }
        if let Ok(v) = serde_json::from_str::<Version>(s)
            && v.version != IR_VERSION
        {
            return Err(format!(
                "IR version {} (this interpreter reads {IR_VERSION})",
                v.version
            ));
        }
        let p: Program = serde_json::from_str(s).map_err(|e| format!("IR: {e}"))?;
        p.validate()?;
        Ok(p)
    }

    /// Slot of an attribute by name.
    pub fn attr_slot(&self, name: &str) -> Option<usize> {
        self.attrs.iter().position(|a| a == name)
    }

    /// Index of an observation by name.
    pub fn observe_slot(&self, name: &str) -> Option<usize> {
        self.observes.iter().position(|a| a == name)
    }

    /// Index of a pool by name.
    pub fn pool_index(&self, name: &str) -> Option<usize> {
        self.pools.iter().position(|p| p.name == name)
    }

    /// Index of a stage by name.
    pub fn stage_index(&self, name: &str) -> Option<usize> {
        self.stages.iter().position(|s| s.name == name)
    }

    /// Replace the workload's arrivals by explicit sessions given as
    /// attribute-name / value pairs.
    pub fn with_sessions(mut self, sessions: &[Vec<(&str, f64)>]) -> Result<Program, String> {
        let mut out = vec![];
        for s in sessions {
            let mut init = SessionInit::default();
            for (name, v) in s {
                let slot = self
                    .attr_slot(name)
                    .ok_or_else(|| format!("no attribute `{name}`"))?;
                init.attrs.push((slot, *v));
            }
            out.push(init);
        }
        self.arrival = CArrival::Sessions(out);
        Ok(self)
    }

    /// Replace an ordered trace corpus by explicit sessions with turns, so
    /// that the IR carries its workload instance as data: `n` sessions (the
    /// program's `batch` or `closed` count), session `i` replaying trace
    /// session `i mod len`, each turn setting `new`, `out`, `think`,
    /// `forced`. The program must arrive in a batch at time 0 (arrival
    /// times are the session's business, e.g. a delay of `serial * spacing`).
    pub fn inline_trace(mut self, corpus: &crate::ir::trace::Corpus) -> Result<Program, String> {
        let n = match self.arrival {
            CArrival::Batch(n) => n,
            ref a => return Err(format!("inline_trace needs `arrive batch(n)`, not {a:?}")),
        };
        if !self.trace_ordered {
            return Err("inline_trace needs an ordered trace".into());
        }
        if corpus.sessions.is_empty() {
            return Err("empty trace".into());
        }
        let sessions = (0..n)
            .map(|i| SessionInit {
                attrs: vec![],
                turns: corpus.sessions[i % corpus.sessions.len()]
                    .turns
                    .iter()
                    .map(|t| {
                        vec![
                            (self.slot_new, t.new),
                            (self.slot_out, t.out),
                            (self.slot_think, t.think),
                            (self.slot_forced, t.forced),
                        ]
                    })
                    .collect(),
            })
            .collect();
        self.arrival = CArrival::Sessions(sessions);
        self.trace = None;
        self.trace_ordered = false;
        Ok(self)
    }

    /// Structural checks: every index refers to something that exists, and
    /// every context variable is read at the moment that supplies it. The
    /// linker runs it on what it produces; it also guards IR read from JSON
    /// or built by tools.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_located().map_err(|e| e.placed(|_, _| None))
    }

    /// `validate`, with the statement an error is about, which the linker
    /// maps back to the program's text (#279).
    pub fn validate_located(&self) -> Result<(), Invalid> {
        self.validate_statements()?;
        self.stage_shares()?;
        self.validate_declarations()?;
        self.validate_types()?;
        // token counts are whole numbers (#661)
        self.validate_whole()?;
        // last, so that a program wrong in another way is told that first
        if let Some(k) = self.unreached_stage() {
            return Err(self.unreached_message(k).into());
        }
        Ok(())
    }

    /// The checks of the blocks and their statements.
    fn validate_statements(&self) -> Result<(), Invalid> {
        if self.version != IR_VERSION {
            return Err(format!(
                "IR version {} (this interpreter reads {IR_VERSION})",
                self.version
            )
            .into());
        }
        self.blocks_are_a_tree()?;
        let v = Validator { p: self };
        for (i, b) in self.blocks.iter().enumerate() {
            // a text program has no block numbers: name the block by its role
            // (every block that is not `init` or `turn` belongs to the session)
            let role = if i == self.init {
                "init"
            } else if i == self.turn {
                "turn"
            } else {
                "session"
            };
            for (k, s) in b.iter().enumerate() {
                v.stmt(s)
                    .map_err(|e| Invalid::at(format!("{role}: {e}"), i, k))?;
            }
        }
        for (k, blk) in [
            ("init", self.init),
            ("turn", self.turn),
            ("session", self.session),
        ] {
            v.block(blk).map_err(|e| format!("{k}: {e}"))?;
        }
        // the workload draws marks; time passes in the session (#272)
        for (k, blk) in [("init", self.init), ("turn", self.turn)] {
            if let Some(at) = self.blocks[blk]
                .iter()
                .position(|s| !matches!(s, CStmt::Set(..) | CStmt::Observe(..)))
            {
                return Err(Invalid::at(
                    format!("{k}: workload blocks may only `set` and `observe`"),
                    blk,
                    at,
                ));
            }
        }
        let leased: Vec<&CRef> = self
            .blocks
            .iter()
            .flatten()
            .filter_map(|s| match s {
                CStmt::Hold {
                    lease: Some((r, _)),
                    ..
                } => Some(r),
                _ => None,
            })
            .collect();
        // a connector's load computes into the request's hold on the
        // engine's memory, and its save runs inside every pool it takes
        for (st, c) in self.connectors() {
            let pools: Vec<&CRef> = st
                .request
                .iter()
                .flat_map(|r| r.pools.iter().map(|(q, _, _)| q))
                .collect();
            let fail = |e: Invalid| Invalid {
                message: format!("a connector: {}", e.message),
                ..e
            };
            if let Some((_, b)) = &c.load {
                let mut held: Vec<_> = pools
                    .iter()
                    .filter(|q| Some(q.base) == st.memory)
                    .map(|q| (*q, None))
                    .collect();
                // a KV relation's load ends the lease its request's entry
                // took `from` the source, the member `E.from` names
                let mut leased = leased.clone();
                if c.from.is_some() {
                    leased.extend(self.blocks[*b].iter().filter_map(|s| match s {
                        CStmt::Release(r) => Some(r),
                        _ => None,
                    }));
                }
                self.enclosed(*b, &mut held, &leased).map_err(fail)?;
            }
            if let Some((_, b)) = &c.save {
                let mut held: Vec<_> = pools.iter().map(|q| (*q, None)).collect();
                self.enclosed(*b, &mut held, &leased).map_err(fail)?;
            }
        }
        self.enclosed(self.session, &mut vec![], &leased)
            .map_err(|e| Invalid {
                message: format!("session: {}", e.message),
                ..e
            })?;
        Ok(())
    }

    /// A register is read where its stage's iteration orders the read: in
    /// the stage's own expressions (budget, chunk, cost, serve keys, its
    /// body, a claim over its iterations), in those of a pool it admits
    /// (`admit_via`: queue, eviction and preempt keys, a spill, a hold's
    /// header whose pools it admits), and in a gauge or a claim `at end`.
    /// Elsewhere — another stage's, a `ps` capacity, a pool admitted at
    /// settle time — the read and the set happen at one instant in an order
    /// that is the order of the declarations, and the answer would be that
    /// order's (#367).
    fn registers_read_in_place(&self) -> Result<(), String> {
        if self.registers.is_empty() {
            return Ok(());
        }
        let read = |e: &CExpr, ok: &dyn Fn(usize) -> bool, place: &str| -> Result<(), String> {
            let Some(CExpr::Reg(r)) = e.find(&|x| matches!(x, CExpr::Reg(r) if !ok(*r))) else {
                return Ok(());
            };
            let reg = &self.registers[*r];
            let owner = &self.stages[reg.stage].name;
            Err(format!(
                "`{}` is engine `{owner}`'s register, and {place} would read it apart from \
                 `{owner}`'s iteration, in an order the declarations would decide; a register \
                 is read by its engine, the keys of a pool it admits, the header of a hold whose \
                 first pool (where it waits) it admits, a gauge or a claim",
                reg.name
            ))
        };
        let of = |stage: usize| move |r: usize| self.registers[r].stage == stage;
        let admitted_by = |pool: usize| {
            move |r: usize| self.pools[pool].admit_via == Some(self.registers[r].stage)
        };
        for (pi, p) in self.pools.iter().enumerate() {
            let place = format!("pool `{}`'s keys", p.name);
            let ok = admitted_by(pi);
            let mut exprs: Vec<&CExpr> = vec![];
            exprs.extend(p.queue.iter().flatten());
            if let CEvict::By(keys) = &p.evict {
                exprs.extend(keys);
            }
            if let Some(keys) = p.preempt.keys() {
                exprs.extend(keys);
            }
            exprs.extend(p.preempt.suspend_keys().into_iter().flatten());
            if let Some(s) = &p.spill {
                exprs.extend([&s.work, &s.when]);
            }
            for e in exprs {
                read(e, &ok, &place)?;
            }
        }
        fn body_exprs<'a>(body: &'a [CIter], out: &mut Vec<&'a CExpr>) {
            for s in body {
                match s {
                    CIter::Serve { only, by } => {
                        out.extend(only.iter());
                        out.extend(by.iter().flatten());
                    }
                    CIter::Admit { only, gate } => out.extend(only.iter().chain(gate.iter())),
                    CIter::Branch(g, a, b) => {
                        out.push(g);
                        body_exprs(a, out);
                        body_exprs(b, out);
                    }
                    CIter::Set(_, e) => out.push(e),
                }
            }
        }
        for (si, s) in self.stages.iter().enumerate() {
            let place = format!("{} `{}`", s.kind.noun(), s.name);
            let ok = of(si);
            match &s.kind {
                CStageKind::Ps(e) => read(e, &|_| false, &format!("{place}'s capacity"))?,
                CStageKind::Step(st) => {
                    let mut exprs: Vec<&CExpr> = vec![&st.budget, &st.chunk, &st.cost];
                    if let CServe::By(keys) = &st.serve {
                        exprs.extend(keys);
                    }
                    if let Some(body) = &st.iteration {
                        body_exprs(body, &mut exprs);
                    }
                    for e in exprs {
                        read(e, &ok, &place)?;
                    }
                    // the admission is a hold's header: the request waits in
                    // its first pool, and the stage that admits that pool
                    // reads it
                    if let Some(r) = &st.request
                        && let Some((queue, _, _)) = r.pools.first()
                    {
                        let ok = |reg: usize| {
                            (queue.base..queue.base + queue.count)
                                .all(|m| self.pools[m].admit_via == Some(self.registers[reg].stage))
                        };
                        let place = format!("{place}'s admission");
                        // its connector's load is read at the same instant
                        let load = st
                            .connector
                            .iter()
                            .filter_map(|c| c.load.as_ref().map(|(e, _)| e));
                        let admission = r
                            .pools
                            .iter()
                            .flat_map(|(_, u, f)| std::iter::once(u).chain(f))
                            .chain(&r.reuse)
                            .chain(r.prefix.iter().flatten().flat_map(|(k, n)| [k, n]))
                            .chain(load);
                        for e in admission {
                            read(e, &ok, &place)?;
                        }
                    }
                }
                CStageKind::Fifo(_) => {}
            }
        }
        for b in &self.blocks {
            for s in b {
                if let CStmt::Hold { pools, reuse, .. } = s {
                    // a hold waits in its first pool's queue, and the stage
                    // that admits that queue reads its header (the other
                    // pools are only tested), so that is the stage whose
                    // registers it may read (a hold has a pool: the parser
                    // requires one)
                    let Some((queue, _, _)) = pools.first() else {
                        continue;
                    };
                    let first: Vec<usize> = (queue.base..queue.base + queue.count).collect();
                    let ok = |reg: usize| {
                        first
                            .iter()
                            .all(|&m| self.pools[m].admit_via == Some(self.registers[reg].stage))
                    };
                    for (_, units, reserve) in pools {
                        read(units, &ok, "a hold's header")?;
                        if let Some(r) = reserve {
                            read(r, &ok, "a hold's header")?;
                        }
                    }
                    if let Some(r) = reuse {
                        read(r, &ok, "a hold's header")?;
                    }
                }
            }
        }
        for c in &self.claims {
            if let Some(st) = c.kind.stage() {
                read(&c.expr, &of(st), &format!("claim `{}`", c.name))?;
            }
        }
        Ok(())
    }

    /// `preempt leases by` names a pool whose leases it may take: one that
    /// some hold leases, and no engine's memory, where a lease is KV another
    /// instance has still to read (a prefiller's blocks before the
    /// decoder's transfer) and the engine's own rule is `preempt by`. Its
    /// `every` is a positive number of seconds, on a pool without `reserve
    /// held`, whose promise a growth let past the cap would break;
    /// `suspend` needs `every`, since a set-aside lease comes back at a
    /// tick.
    fn validate_lease_preemption(&self) -> Result<(), String> {
        for (pi, p) in self.pools.iter().enumerate() {
            let Preempt::Leases { every, suspend, .. } = &p.preempt else {
                continue;
            };
            let at = |e: &str| format!("pool `{}`: `preempt leases by` {e}", p.name);
            if let Some(dt) = every
                && !(dt.is_finite() && *dt > 0.0)
            {
                return Err(at(&format!(
                    "`every ({dt})`: a tick is a positive, finite number of seconds"
                )));
            }
            if every.is_some() && p.reserve_held {
                return Err(at(
                    "`every` on a pool with `reserve held`: a growth let past the cap until \
                     the tick would take what the live holds reserved",
                ));
            }
            if suspend.is_some() && every.is_none() {
                return Err(at(
                    "`suspend` without `every`: a lease set aside comes back at a tick, and \
                     without ticks it would never come back",
                ));
            }
            let memory = self
                .stages
                .iter()
                .any(|st| matches!(&st.kind, CStageKind::Step(s) if s.memory == Some(pi)));
            if memory {
                return Err(at(
                    "on an engine's memory: a lease there is KV another instance has still \
                     to read, which taking would lose; the engine preempts its residents by \
                     `preempt by`",
                ));
            }
            let leased = self.blocks.iter().flatten().any(|s| {
                matches!(s, CStmt::Hold { lease: Some((r, _)), .. }
                    if (r.base..r.base + r.count.max(1)).contains(&pi))
            });
            if !leased {
                return Err(at(
                    "on a pool no hold leases: it has nothing to take, and a growth there \
                     waits as under `preempt none`",
                ));
            }
        }
        Ok(())
    }

    /// The first stage no statement runs: the stage of no `run` (nor one of
    /// its `also`) and of no engine `request`, in any block, a connector's
    /// among them, nor the link a pool spills `via`. No request reaches it,
    /// so it has no arrow in and the deployment view leaves it out.
    pub fn unreached_stage(&self) -> Option<usize> {
        let mut reached = vec![false; self.stages.len()];
        for s in self.pools.iter().filter_map(|p| p.spill.as_ref()) {
            reached[s.via] = true;
        }
        for st in self.blocks.iter().flatten() {
            match st {
                CStmt::Run { stage, also, .. } => {
                    for r in std::iter::once(stage).chain(also) {
                        reached[r.base..r.base + r.count].fill(true);
                    }
                }
                CStmt::Request { stage, .. } => {
                    reached[stage.base..stage.base + stage.count].fill(true);
                }
                _ => {}
            }
        }
        reached.iter().position(|r| !r)
    }

    /// Why stage `k`, which no statement runs (`unreached_stage`), does not
    /// link, and what to write instead.
    pub fn unreached_message(&self, k: usize) -> String {
        let name = &self.stages[k].name;
        let what = self.stages[k].kind.noun();
        match name.strip_suffix(".nic") {
            Some(queue) => format!(
                "{}\nhelp: remove the `nic` of `{queue}` (and what reads `{name}`), or copy the \
                 KV over it: `kvt.nixl_pull` or `kvt.nixl_push` between `{queue}` and another \
                 queue with a `nic`",
                crate::frontend::diagnostic::Code::NicUnused.message(format_args!(
                    "the NIC `{name}` carries no copy: no relation or `transfer` runs on it, so \
                     no request reaches it and the deployment view would leave it out"
                ))
            ),
            None => format!(
                "{what} `{name}` is run by no statement: no request reaches it, so the \
                 deployment view would leave it out\n\
                 help: run it, or call an entry of its queue, from the server or a gateway's \
                 `route`; or remove it"
            ),
        }
    }

    /// The checks of the declarations and the run.
    fn validate_declarations(&self) -> Result<(), String> {
        let v = Validator { p: self };
        self.validate_flows()?;
        self.validate_lease_preemption()?;
        for p in &self.pools {
            let at = |e| format!("pool `{}`: {e}", p.name);
            if let Some(dt) = p.queue_every {
                if !(dt.is_finite() && dt > 0.0) {
                    return Err(at(format!(
                        "`queue … every ({dt})`: a tick is a positive, finite number of seconds"
                    )));
                }
                if p.admit_via.is_some() {
                    return Err(at(
                        "`queue … every` on a pool an engine admits: the engine's schedule \
                         admits its queue at each iteration"
                            .to_string(),
                    ));
                }
            }
            if let Some(keys) = &p.queue {
                if keys.is_empty() {
                    return Err(at("queue by needs at least one key".to_string()));
                }
                for key in keys {
                    v.expr(key, Moment::Select).map_err(at)?;
                    if draws(key) {
                        return Err(at(
                            "a queue key may not draw; sample into an attribute first".to_string(),
                        ));
                    }
                }
            }
            let form = match p.preempt {
                Preempt::Leases { .. } => "preempt leases by",
                _ => "preempt by",
            };
            // the victim's keys, and those a set-aside lease comes back in
            let groups = [
                (p.preempt.keys(), form),
                (p.preempt.suspend_keys(), "suspend by"),
            ];
            for (keys, form) in groups {
                let Some(keys) = keys else { continue };
                if keys.is_empty() {
                    return Err(at(format!("{form} needs at least one key")));
                }
                for key in keys {
                    v.expr(key, Moment::Victim).map_err(at)?;
                    if draws(key) {
                        return Err(at(
                            "a preempt key may not draw (`~`): it is read for every candidate \
                             at every growth that does not fit; sample into an attribute first"
                                .to_string(),
                        ));
                    }
                    if key.any(&|x| matches!(x, CExpr::Attr(a) if *a == self.slot_computed)) {
                        return Err(at(
                            "a preempt key reads `computed`, the position at the session's last \
                             preemption (0 for one never preempted); a preempt key reads \
                             `position`, where the candidate is now"
                                .to_string(),
                        ));
                    }
                }
            }
            if let CEvict::By(keys) = &p.evict {
                for e in keys {
                    v.expr(e, Moment::Evict).map_err(at)?;
                }
            }
            if let Some(s) = &p.spill {
                v.pool(s.to)?;
                v.stage(s.via)?;
                v.expr(&s.work, Moment::Evict).map_err(at)?;
                v.expr(&s.when, Moment::Evict).map_err(at)?;
            }
            if let Some(s) = p.admit_via {
                v.stage(s)?;
                // only a step stage's scheduler admits a bound queue
                // (`admit_bound`); named for any other stage, the queue is
                // never served (#418)
                let stage = &self.stages[s];
                if !matches!(stage.kind, CStageKind::Step(_)) {
                    return Err(at(format!(
                        "`admit_via` names `{}`, but `{}` is a {} stage: only an engine's `scheduler` \
                         admits, and nothing would admit `{}`",
                        stage.name,
                        stage.name,
                        stage.kind.word(),
                        p.name
                    )));
                }
            }
        }
        for (si, s) in self.stages.iter().enumerate() {
            let at = |e| format!("{} `{}`: {e}", s.kind.noun(), s.name);
            match &s.kind {
                CStageKind::Fifo(_) => {}
                CStageKind::Ps(e) => v.expr(e, Moment::Ps).map_err(at)?,
                CStageKind::Step(st) => {
                    v.expr(&st.budget, Moment::Budget).map_err(at)?;
                    v.expr(&st.chunk, Moment::Budget).map_err(at)?;
                    v.expr(&st.cost, Moment::Step).map_err(at)?;
                    if let Some(r) = &st.request {
                        v.request(r).map_err(at)?;
                    }
                    if let Some(c) = &st.connector {
                        v.connector(st, c).map_err(at)?;
                    }
                    if let Some(g) = &st.granule {
                        let g = match g {
                            CExpr::Num(g) if *g > 0.0 => *g,
                            _ => {
                                return Err(at(format!(
                                    "granule {}: a constant above 0, `inf` included",
                                    self.show_expr(g)
                                )));
                            }
                        };
                        if matches!(st.serve, CServe::ExclusivePrefill) {
                            return Err(at(
                                "`granule` with `exclusive prefill`: a prefill the granule \
                                 refuses would still block every decode, and the engine would \
                                 stop"
                                    .into(),
                            ));
                        }
                        // a prefill longer than a constant chunk only ever gets
                        // part of it, at most the chunk, which the granule must
                        // allow (TensorRT-LLM refuses a chunk below its unit,
                        // microBatchScheduler.cpp L278-L282); a prompt longer
                        // than the budget is the workload's, as it is there
                        if let Some(c) = constant(&st.chunk)
                            && c > 0.0
                            && g > c
                        {
                            return Err(at(format!(
                                "granule {g} with `each at most ({c})`: a prompt longer than \
                                 {c} tokens would never get a token"
                            )));
                        }
                        // a `max(k, e)` chunk can always fall to its floor `k`,
                        // where the same prompt gets nothing (#448)
                        if let CExpr::Call(Fun::Max, args) = &st.chunk
                            && let Some(k) = args
                                .iter()
                                .filter_map(|a| match a {
                                    CArg::Expr(e) => constant(e),
                                    _ => None,
                                })
                                .reduce(f64::max)
                            && k > 0.0
                            && g > k
                        {
                            return Err(at(format!(
                                "granule {g} with chunk max({k}, …): a prompt longer than {k} \
                                 would get no token in an iteration whose chunk is {k}"
                            )));
                        }
                    }
                    if let CServe::By(keys) = &st.serve {
                        keys.iter().try_for_each(|k| v.serve_key(k)).map_err(at)?;
                    }
                    if let Some(body) = &st.iteration {
                        if matches!(st.serve, CServe::ExclusivePrefill) {
                            return Err(at(
                                "`exclusive prefill` with a `scheduler` body (the IR's `iteration`): \
                                 the rule takes back decodes already chosen, which a body \
                                 cannot, and the two would answer one question twice"
                                    .into(),
                            ));
                        }
                        v.iteration(si, body).map_err(at)?;
                        if !always_serves(body) {
                            return Err(at(
                                "a `scheduler` with a path that neither advances `running` nor \
                                 admits `waiting`: an engine whose schedule takes that path \
                                 schedules nothing, and waits for an event that may never come"
                                    .into(),
                            ));
                        }
                    }
                    if let Some(m) = st.memory {
                        v.pool(m)?;
                    }
                }
            }
        }
        self.registers_read_in_place()?;
        for (k, r) in self.registers.iter().enumerate() {
            let at = |e: &str| format!("state `{}`: {e}", r.name);
            match self.stages.get(r.stage).map(|s| &s.kind) {
                Some(CStageKind::Step(st)) if st.iteration.is_some() => {}
                Some(CStageKind::Step(_)) => {
                    return Err(at(
                        "a register of an engine without a `scheduler` body (the IR's `iteration`), \
                         which nothing sets",
                    ));
                }
                _ => return Err(at("a register belongs to an engine (the IR's step stage)")),
            }
            if !r.init.is_finite() {
                return Err(at("its first value is a finite number"));
            }
            if self.registers[..k].iter().any(|q| q.name == r.name) {
                return Err(at("declared twice"));
            }
        }
        for (k, g) in self.gauges.iter().enumerate() {
            let at = |e| format!("gauge `{}`: {e}", g.name);
            v.expr(&g.expr, Moment::Gauge).map_err(at)?;
            if self.gauges[..k].iter().any(|h| h.name == g.name) {
                return Err(at("declared twice".into()));
            }
            if self.observes.contains(&g.name) {
                return Err(at(
                    "is also an `observe`: one name would be two statistics".into()
                ));
            }
        }
        for (k, c) in self.claims.iter().enumerate() {
            let at = |e| format!("claim `{}`: {e}", c.name);
            if self.claims[..k].iter().any(|d| d.name == c.name) {
                return Err(at("declared twice".into()));
            }
            if let Some(g) = &c.given {
                v.expr(g, Moment::Given).map_err(at)?;
            }
            if let Some(st) = c.kind.stage() {
                v.stage(st).map_err(at)?;
                if !matches!(self.stages[st].kind, CStageKind::Step(_)) {
                    let s = &self.stages[st];
                    let name = match s.index {
                        Some(i) => format!("{}[{i}]", s.name),
                        None => s.name.clone(),
                    };
                    return Err(at(format!(
                        "stage `{name}` is not an engine: only an engine has iterations"
                    )));
                }
            }
            v.expr(&c.expr, c.kind.moment()).map_err(at)?;
        }
        if let CArrival::Renewal(e) = &self.arrival {
            v.expr(e, Moment::Session)?;
            if !arrival_expr_is_pure(e) {
                return Err(
                    "renewal arrival may use only constants and sampled distributions".into(),
                );
            }
            if let CExpr::Num(gap) = e
                && !(gap.is_finite() && *gap > 0.0)
            {
                return Err(format!(
                    "`arrive renewal({gap})`: an interarrival time must be positive"
                ));
            }
        }
        // a rate that is not positive draws gaps that are not: time runs
        // backwards and the run never ends (#286)
        if let CArrival::Poisson(rate) = self.arrival
            && !(rate.is_finite() && rate > 0.0)
        {
            return Err(format!(
                "`arrive poisson(…)`: the rate is {rate}; a rate must be positive and finite"
            ));
        }
        // a workload starts its sessions before the run begins: a count is
        // at least one and at most what can be made (#289)
        if let CArrival::Closed(n) | CArrival::Batch(n) = self.arrival
            && !(1..=MAX_SESSIONS).contains(&n)
        {
            return Err(format!(
                "workload: {n} sessions: a closed population or a batch is from 1 to {MAX_SESSIONS}"
            ));
        }
        if let Some(n) = self.arrivals {
            if n == 0 {
                return Err("run: arrivals must be positive".into());
            }
            if !matches!(self.arrival, CArrival::Poisson(_) | CArrival::Renewal(_)) {
                return Err("run: arrivals applies only to poisson or renewal workloads".into());
            }
        }
        if let CArrival::Sessions(ss) = &self.arrival {
            for s in ss {
                for (slot, _) in s.attrs.iter().chain(s.turns.iter().flatten()) {
                    v.attr(*slot)?;
                }
            }
        }
        for slot in [
            self.slot_cached,
            self.slot_serial,
            self.slot_turn,
            self.slot_new,
            self.slot_out,
            self.slot_think,
            self.slot_more,
            self.slot_forced,
            self.slot_computed,
        ] {
            v.attr(slot)?;
        }
        for (k, &slot) in self.hidden.iter().enumerate() {
            v.attr(slot).map_err(|e| format!("hidden: {e}"))?;
            // what the scheduler writes it cannot be kept from reading
            if slot == self.slot_cached || slot == self.slot_computed {
                return Err(format!(
                    "hidden `{}`: the scheduler sets it, so it cannot be hidden from the scheduler",
                    self.attrs[slot]
                ));
            }
            if self.hidden[..k].contains(&slot) {
                return Err(format!("hidden `{}` twice", self.attrs[slot]));
            }
        }
        if !(self.horizon.is_finite() && self.warmup >= 0.0 && self.warmup < self.horizon) {
            return Err("run: need 0 <= warmup < horizon".into());
        }
        Ok(())
    }
}

/// Whether every path through an iteration body reaches a `serve` or an
/// `admit`.
fn always_serves(body: &[CIter]) -> bool {
    body.iter().any(|s| match s {
        CIter::Serve { .. } | CIter::Admit { .. } => true,
        CIter::Branch(_, a, b) => always_serves(a) && always_serves(b),
        CIter::Set(..) => false,
    })
}

/// Whether an expression samples a distribution anywhere.
fn draws(e: &CExpr) -> bool {
    e.any(&|x| matches!(x, CExpr::Sample(..)))
}

/// Whether an expression reads a value that moves while no event happens
/// (`now`, `work(…)`).
fn reads_clock(e: &CExpr) -> bool {
    e.any(&|x| matches!(x, CExpr::Ctx(CtxVar::Now) | CExpr::Call(Fun::Work, _)))
}

/// The value of an expression of numbers and operators, if it is one.
fn constant(e: &CExpr) -> Option<f64> {
    match e {
        CExpr::Num(x) => Some(*x),
        CExpr::Cost(_, x) => constant(x),
        CExpr::Unary(UnOp::Neg, x) => constant(x).map(|x| -x),
        CExpr::Binary(op, a, b) => Some(crate::frontend::link::binop(
            *op,
            constant(a)?,
            constant(b)?,
        )),
        CExpr::Cond(c, a, b) => {
            if constant(c)? != 0.0 {
                constant(a)
            } else {
                constant(b)
            }
        }
        // the linker's constant functions (`min`, `max`, …) of constants
        CExpr::Call(f, args) => {
            let xs = args
                .iter()
                .map(|a| match a {
                    CArg::Expr(x) => constant(x),
                    CArg::Pool(_) | CArg::Stage(_) => None,
                })
                .collect::<Option<Vec<f64>>>()?;
            f.arithmetic(&xs)
        }
        _ => None,
    }
}

/// An amount a statement names (units, tokens, seconds) is not negative,
/// and not NaN: a constant one that is does not link; a computed one fails
/// the run (#270).
fn amount(e: &CExpr, what: &str) -> Result<(), String> {
    match constant(e) {
        Some(x) if x.is_nan() || x < 0.0 => Err(format!(
            "`{what} ({x})`: an amount of units, tokens or seconds is a number, and not negative"
        )),
        _ => Ok(()),
    }
}

/// Whether an expression's value moves while a session holds still: it
/// reads the clock, the context, the state or a draw. An index that does
/// is not the same member when read again (#282, #317), and a hold's units
/// that do ask for something else at the next try (#364).
pub(crate) fn moves(e: &CExpr) -> bool {
    e.any(&|x| match x {
        // a register moves at its stage's iterations (#377: read at the join,
        // a reserve on one was judged on a value an iteration then lowered)
        // a request's input, output and tokens stand still while it waits
        CExpr::Ctx(CtxVar::Input | CtxVar::Output | CtxVar::Known) => false,
        CExpr::Ctx(_) | CExpr::Reg(_) | CExpr::Sample(..) => true,
        // a function of its arguments alone does not move
        CExpr::Call(f, _) => !f.is_arithmetic(),
        _ => false,
    })
}

/// Whether a renewal gap reads only constants and draws: no attribute, no
/// context variable, no pool or stage.
fn arrival_expr_is_pure(e: &CExpr) -> bool {
    !e.any(&|x| match x {
        CExpr::Attr(_) | CExpr::Ctx(_) => true,
        CExpr::Call(_, args) => args.iter().any(|a| !matches!(a, CArg::Expr(_))),
        _ => false,
    })
}

struct Validator<'a> {
    p: &'a Program,
}

impl Validator<'_> {
    /// An `advance running by (…)` key, in either form of `scheduler`: read
    /// for every resident at every iteration, so it does not draw. It may
    /// read the clock: a key orders the residents and excludes none.
    fn serve_key(&self, k: &CExpr) -> Result<(), String> {
        self.expr(k, Moment::Serve)?;
        if draws(k) {
            return Err(
                "an `advance running by (…)` key may not draw (`~`): it is read for every \
                 resident at every iteration, and the order would change under the \
                 scheduler's feet"
                    .into(),
            );
        }
        Ok(())
    }

    /// An `only` of `advance running` or `admit waiting`: read as a key is,
    /// and not reading the clock either, since an engine whose residents it
    /// all excludes waits for an event, and the clock moving is none (#263).
    fn only(&self, e: &CExpr) -> Result<(), String> {
        self.expr(e, Moment::Serve)?;
        if draws(e) {
            return Err(
                "an `only` may not draw (`~`): it is read for every resident at every \
                 iteration"
                    .into(),
            );
        }
        if reads_clock(e) {
            return Err(
                "an `only` may not read `now` or `work(…)`: an engine whose requests it \
                 all excludes waits for the next event, and time passing is not one"
                    .into(),
            );
        }
        Ok(())
    }

    /// A step stage's iteration body: its keys and `only`s as above, a
    /// guard and an `admit`'s `while` as the iteration is planned, neither
    /// drawing nor reading the clock (an engine whose `scheduler` schedules
    /// nothing waits for an event, and the clock moving is none, #263), and
    /// a guard does not read this stage's `budget_left`, which plans the
    /// iteration the body is planning.
    fn iteration(&self, st: usize, body: &[CIter]) -> Result<(), String> {
        let plan = |e: &CExpr, what: &str| -> Result<(), String> {
            self.expr(e, Moment::Plan)?;
            if draws(e) {
                return Err(format!(
                    "{what} may not draw (`~`): it is read at every iteration"
                ));
            }
            if reads_clock(e) {
                return Err(format!(
                    "{what} may not read `now` or `work(…)`: an engine whose `scheduler` \
                     schedules nothing waits for an event, and the clock moving is none"
                ));
            }
            let own = |r: &CRef| (r.base..r.base + r.count).contains(&st);
            if e.any(&|x| {
                matches!(x, CExpr::Call(Fun::BudgetLeft, a)
                    if matches!(a.as_slice(), [CArg::Stage(r)] if own(r)))
            }) {
                return Err(format!(
                    "{what} may not read this engine's `budget_left(…)`: it plans an \
                     iteration, and the schedule is the plan"
                ));
            }
            Ok(())
        };
        for s in body {
            match s {
                CIter::Serve { only, by } => {
                    only.iter().try_for_each(|e| self.only(e))?;
                    by.iter().flatten().try_for_each(|k| self.serve_key(k))?;
                }
                CIter::Admit { only, gate } => {
                    only.iter().try_for_each(|e| self.only(e))?;
                    if let Some(g) = gate {
                        plan(g, "`admit waiting while (…)`")?;
                    }
                }
                CIter::Branch(g, a, b) => {
                    plan(g, "an `if` in a `scheduler`")?;
                    self.iteration(st, a)?;
                    self.iteration(st, b)?;
                }
                CIter::Set(r, e) => {
                    let Some(reg) = self.p.registers.get(*r) else {
                        return Err(format!("register {r} out of range"));
                    };
                    if reg.stage != st {
                        return Err(format!(
                            "`set {}`: the register is engine `{}`'s; a schedule sets its own engine's",
                            reg.name, self.p.stages[reg.stage].name
                        ));
                    }
                    plan(e, &format!("`set {}`", reg.name))?;
                }
            }
        }
        Ok(())
    }

    fn block(&self, b: BlockId) -> Result<(), String> {
        if b < self.p.blocks.len() {
            Ok(())
        } else {
            Err(format!("block {b} out of range"))
        }
    }
    fn attr(&self, a: usize) -> Result<(), String> {
        if a < self.p.attrs.len() {
            Ok(())
        } else {
            Err(format!("attribute slot {a} out of range"))
        }
    }
    /// `serial` is the session's number, which the run gives it: a session's
    /// cache entry is what hangs under the key of its serial, and a program
    /// that set one would hand its context to another session by value.
    fn serial_unset(&self, a: usize) -> Result<(), String> {
        if a == self.p.slot_serial {
            Err(
                "`serial` is the session's number, which the run assigns; set another \
                 attribute"
                    .into(),
            )
        } else {
            Ok(())
        }
    }
    fn observe(&self, a: usize) -> Result<(), String> {
        if a < self.p.observes.len() {
            Ok(())
        } else {
            Err(format!("observation {a} out of range"))
        }
    }
    fn pool(&self, a: usize) -> Result<(), String> {
        if a < self.p.pools.len() {
            Ok(())
        } else {
            Err(format!("pool {a} out of range"))
        }
    }
    fn stage(&self, a: usize) -> Result<(), String> {
        if a < self.p.stages.len() {
            Ok(())
        } else {
            Err(format!("stage {a} out of range"))
        }
    }
    /// A pool or stage reference; its index is evaluated with the expression
    /// around it, so at the same moment.
    fn cref(&self, r: &CRef, n: usize, what: &str, m: Moment) -> Result<(), String> {
        if r.count == 0 || r.base + r.count > n {
            return Err(format!(
                "{what} reference {}..{} out of range",
                r.base,
                r.base + r.count
            ));
        }
        // a gauge or a claim names its members by number (an aggregate
        // writes them so): an index read from the state could leave the
        // array, and a gauge or a claim that fails the run would be one a
        // reader ignoring them does not
        if matches!(m, Moment::Gauge | Moment::Iteration)
            && let Some(e) = &r.index
            && !matches!(**e, CExpr::Num(k) if k >= 0.0 && k.fract() == 0.0 && k < r.count as f64)
        {
            let whose = if m == Moment::Gauge {
                "a gauge"
            } else {
                "a claim"
            };
            return Err(format!(
                "{whose}'s {what} index is a number in range (`kv[0]`, or an aggregate's `kv[k]`), \
                 not one read from the state"
            ));
        }
        if let Some(e) = &r.index {
            if let Some(k) = constant(e)
                && !(k >= 0.0 && k.fract() == 0.0 && k < r.count as f64)
            {
                return Err(format!(
                    "{what} index {k}: a member of an array of {} is 0 to {}",
                    r.count,
                    r.count - 1
                ));
            }
            self.expr(e, m)?;
        }
        Ok(())
    }
    /// The expression is well formed and reads only what moment `m` supplies.
    fn expr(&self, e: &CExpr, m: Moment) -> Result<(), String> {
        match e {
            CExpr::Cost(target, value) => {
                self.p.validate_cost_target(target)?;
                self.expr(value, m)
            }
            CExpr::Num(_) => Ok(()),
            // a gauge is integrated as constant between events: what moves
            // between them would be read at the event and held
            CExpr::Ctx(CtxVar::Now) if m == Moment::Gauge => Err(
                "a gauge may not read `now`: it changes between events, and a gauge is held \
                 constant between them"
                    .into(),
            ),
            CExpr::Ctx(CtxVar::Now) if m == Moment::Given => Err(
                "a claim's `given` may not read `now`: it is a condition on one session's \
                 attributes"
                    .into(),
            ),
            CExpr::Ctx(v) => {
                let at = v.moments();
                if at.is_empty() || at.contains(&m) {
                    Ok(())
                } else {
                    let only: Vec<String> = at.iter().map(|x| x.to_string()).collect();
                    Err(format!(
                        "`{}` is read in {m}, but it exists only in {}",
                        v.name(),
                        only.join(" or ")
                    ))
                }
            }
            CExpr::Attr(a) if m == Moment::Gauge => Err(format!(
                "`{}` is a session attribute, and a gauge has no session",
                self.p.attrs.get(*a).map_or("?", |s| s.as_str())
            )),
            // a stage's capacity, budget, chunk, cost and an iteration body's
            // guard are read for the stage, where no one session is: the
            // interpreter would read the attribute as NaN
            CExpr::Attr(a) if matches!(m, Moment::Ps | Moment::Budget | Moment::Step | Moment::Plan) => {
                Err(format!(
                    "`{}` is a session attribute, and {} has no session",
                    self.p.attrs.get(*a).map_or("?", |s| s.as_str()),
                    m.place()
                ))
            }
            CExpr::Sample(..) if m == Moment::Gauge => Err(
                "a gauge may not draw (`~`): it reads the state, and a draw would move the run's streams"
                    .into(),
            ),
            CExpr::Call(Fun::Work, _) if m == Moment::Gauge => Err(
                "a gauge may not read `work(…)`: the work left drains between events, and a \
                 gauge is held constant between them"
                    .into(),
            ),
            CExpr::Call(Fun::BudgetLeft, _) if m == Moment::Gauge => Err(
                "a gauge may not read `budget_left(…)`: it plans the next iteration, which \
                 evaluates the engine's `tokens cap` and may draw"
                    .into(),
            ),
            // budget_left plans an iteration from a budget: a budget read
            // from it, this engine's or another's, plans from itself (#284)
            CExpr::Call(Fun::BudgetLeft, _) if m == Moment::Budget => Err(
                "an engine's `tokens cap` or `each at most` may not read `budget_left(…)`: \
                 budget_left plans an iteration from a `tokens cap`, so a `tokens cap` that \
                 reads it can read itself"
                    .into(),
            ),
            CExpr::Call(Fun::BudgetLeft, _) if m == Moment::Victim => Err(
                "a preempt key may not read `budget_left(…)`: a victim is chosen while the \
                 iteration that would answer it is being planned"
                    .into(),
            ),
            CExpr::Call(Fun::CachedIn, _) if m == Moment::Gauge => Err(
                "`cachedin` is the session's own cached prefix, and a gauge has no session".into(),
            ),
            CExpr::Call(Fun::Allocated, _) if m == Moment::Gauge => Err(
                "`allocated` is the session's own allocation, and a gauge has no session".into(),
            ),
            // a claim reads and does not act: no draw, and what its moment
            // supplies (a `given` the session, an iteration claim the stage
            // as the iteration starts, a claim `at end` the run)
            CExpr::Sample(..) if matches!(m, Moment::Given | Moment::Iteration | Moment::End) => {
                Err("a claim may not draw (`~`): it reads and does not act, and a draw would \
                     move the run's streams"
                    .into())
            }
            CExpr::Attr(a) if m == Moment::Iteration => Err(format!(
                "`{}` is a session attribute, and a claim over iterations reads the engine, \
                 not a session",
                self.p.attrs.get(*a).map_or("?", |s| s.as_str())
            )),
            CExpr::Attr(a) if m == Moment::End => Err(format!(
                "`{}` is a session attribute, and a claim `at end` reads the run, not a session",
                self.p.attrs.get(*a).map_or("?", |s| s.as_str())
            )),
            CExpr::Call(f, _) if matches!(m, Moment::Given | Moment::End) && !f.is_arithmetic() => {
                Err(format!(
                    "`{}(…)` reads the deployment's state, and {} reads {}",
                    f.name(),
                    m.place(),
                    if m == Moment::Given {
                        "the session's attributes and the constants"
                    } else {
                        "the constants, `now` and the aggregates of the observations"
                    }
                ))
            }
            CExpr::Call(f, _)
                if m == Moment::Iteration
                    && !f.is_arithmetic()
                    && !matches!(
                        f,
                        Fun::Queue | Fun::Busy | Fun::Used | Fun::Free | Fun::Holders | Fun::Queued
                    ) =>
            {
                Err(format!(
                    "a claim over iterations may not read `{}(…)`: it reads the deployment as the \
                     iteration starts through `queue`, `busy`, `used`, `free`, `holders` and \
                     `queued` (`work` drains between events, `budget_left` plans an iteration, \
                     `cachedin` is a session's)",
                    f.name()
                ))
            }
            CExpr::Agg(a, k) => {
                if m != Moment::End {
                    return Err(format!(
                        "`{}(…)` aggregates the run's observations and is read only by a claim \
                         `at end`, but is read in {m}",
                        a.name()
                    ));
                }
                self.observe(*k)
            }
            CExpr::Reg(r) => {
                if *r >= self.p.registers.len() {
                    return Err(format!("register {r} out of range"));
                }
                // a register is the scheduler's: a session and a claim's
                // `given` read the session's, and a request's release, as a
                // hold's `cache`, is the session's
                if matches!(m, Moment::Session | Moment::Given | Moment::Release) {
                    return Err(format!(
                        "`{}` is an engine's register (`state`), which its schedule reads; \
                         it is read in {m}",
                        self.p.registers[*r].name
                    ));
                }
                Ok(())
            }
            CExpr::Attr(a) => {
                self.attr(*a)?;
                // a claim's `given` is no scheduler's: it may read anything
                // the session has
                if !matches!(m, Moment::Session | Moment::Given) && self.p.hidden.contains(a) {
                    return Err(format!(
                        "`{}` is hidden from the scheduler, but is read in {m}",
                        self.p.attrs[*a]
                    ));
                }
                Ok(())
            }
            CExpr::Sample(d, xs) => {
                if xs.len() != d.arity() {
                    return Err(format!(
                        "`~{}` takes {} argument(s), got {}",
                        d.name(),
                        d.arity(),
                        xs.len()
                    ));
                }
                xs.iter().try_for_each(|x| self.expr(x, m))
            }
            CExpr::Call(f, args) => {
                let sig = f.signature();
                if args.len() != sig.len() {
                    return Err(format!(
                        "`{}` takes {} argument(s), got {}",
                        f.name(),
                        sig.len(),
                        args.len()
                    ));
                }
                for (a, k) in args.iter().zip(sig) {
                    let found = match a {
                        CArg::Expr(_) => ArgKind::Expr,
                        CArg::Pool(_) => ArgKind::Pool,
                        CArg::Stage(_) => ArgKind::Stage,
                    };
                    if found != *k {
                        let want = match k {
                            ArgKind::Expr => "an expression",
                            ArgKind::Pool => "a pool",
                            ArgKind::Stage => "a stage",
                        };
                        return Err(format!("`{}` expects {want} here", f.name()));
                    }
                }
                args.iter().try_for_each(|a| match a {
                    CArg::Expr(x) => self.expr(x, m),
                    CArg::Pool(r) => self.cref(r, self.p.pools.len(), "pool", m),
                    CArg::Stage(r) => self.cref(r, self.p.stages.len(), "stage", m),
                })?;
                // the token budget is a step engine's: on any other stage
                // there is none to read (#268)
                if let (Fun::BudgetLeft, [CArg::Stage(r)]) = (f, args.as_slice())
                    && let Some(s) = self.p.stages[r.base..r.base + r.count]
                        .iter()
                        .find(|s| !matches!(s.kind, CStageKind::Step(_)))
                {
                    let kind = s.kind.word();
                    let n = &s.name;
                    let what = if s.index.is_some() {
                        format!("a member of `{n}`")
                    } else {
                        format!("`{n}`")
                    };
                    return Err(format!(
                        "`budget_left({n})`: {what} is a {kind} stage; only an engine has \
                         a token budget (`tokens cap`)"
                    ));
                }
                Ok(())
            }
            CExpr::Unary(_, x) => self.expr(x, m),
            CExpr::Binary(_, a, b) => {
                self.expr(a, m)?;
                self.expr(b, m)
            }
            CExpr::Cond(c, a, b) => {
                self.expr(c, m)?;
                self.expr(a, m)?;
                self.expr(b, m)
            }
        }
    }
    /// Whether every path through block `b` reaches a statement that lets
    /// time pass: a `run` (a constant zero work does not count; a computed
    /// one is the run time's to catch), a `hold` whose body does or that
    /// waits for a queue tick, or `end`.
    /// The condition under which a `loop` blocks on every pass
    /// (`docs/design/stochastic-model.md`, Lemma 1). An `if` counts when
    /// both arms do; an inner `loop` is never left, so it counts when it does.
    fn lets_time_pass(&self, b: BlockId) -> bool {
        for s in &self.p.blocks[b] {
            match s {
                CStmt::End => return true,
                CStmt::Run { work, latency, .. } => {
                    let none = |e: &CExpr| constant(e).is_some_and(|w| w <= 0.0);
                    if !(none(work) && latency.as_ref().is_none_or(none)) {
                        return true;
                    }
                }
                CStmt::Request { input, output, .. } => {
                    let none = |e| constant(e).is_some_and(|w| w <= 0.0);
                    if !(none(input) && none(output)) {
                        return true;
                    }
                }
                CStmt::Hold { pools, body, .. } => {
                    // a hold queued on a pool under `queue … every` is
                    // admitted at a tick, one queued during a tick at the
                    // next: a session at most once an instant per pool, so
                    // a loop through it passes so many times at most
                    let ticks = pools.first().is_some_and(|(r, _, _)| {
                        (r.base..r.base + r.count).all(|i| self.p.pools[i].queue_every.is_some())
                    });
                    if ticks || self.lets_time_pass(*body) {
                        return true;
                    }
                }
                CStmt::Branch(_, a, c) => {
                    if self.lets_time_pass(*a) && self.lets_time_pass(*c) {
                        return true;
                    }
                }
                CStmt::Loop(inner) => return self.lets_time_pass(*inner),
                _ => {}
            }
        }
        false
    }

    /// An engine's request: a hold's header, read for the request at its
    /// admission and its release.
    fn request(&self, r: &CRequest) -> Result<(), String> {
        let np = self.p.pools.len();
        let no_draw = |e: &CExpr, what: &str| {
            if draws(e) {
                Err(format!("a request's {what} may not draw"))
            } else {
                Ok(())
            }
        };
        for (k, (q, u, reserve)) in r.pools.iter().enumerate() {
            self.cref(q, np, "pool", Moment::Request)?;
            if r.pools[..k].iter().any(|(o, _, _)| o == q) {
                return Err(format!(
                    "a request takes `{}` twice; take it once",
                    self.p.show_pool_ref(q)
                ));
            }
            self.expr(u, Moment::Request)?;
            no_draw(u, "units")?;
            if let Some(f) = reserve {
                self.expr(f, Moment::Request)?;
                no_draw(f, "`reserve`")?;
            }
        }
        if let Some(e) = &r.reuse {
            self.expr(e, Moment::Request)?;
            no_draw(e, "`reuse`")?;
        }
        // `request.hit` is `min(cachedin(P), reuse)`, and the connector's
        // load is read after it: neither is there for `reuse` to read (a
        // `prefix` segment reads no state at all, below)
        if let Some(CExpr::Ctx(v)) = r
            .reuse
            .as_ref()
            .and_then(|e| e.find(&|x| matches!(x, CExpr::Ctx(CtxVar::Hit | CtxVar::Loaded))))
        {
            return Err(format!(
                "a request's `reuse` reads `{}`, which the admission computes from it",
                v.name()
            ));
        }
        if let Some(e) = &r.cache {
            self.expr(e, Moment::Release)?;
        }
        for (k, n) in r.prefix.iter().flatten() {
            for e in [k, n] {
                self.expr(e, Moment::Request)?;
                if moves(e) {
                    return Err(
                        "a request's `prefix` segment reads the state, the clock or a draw; a \
                         key names content, an attribute or a number"
                            .into(),
                    );
                }
            }
        }
        let rec = r.records;
        for k in [rec.reused, rec.prefill] {
            if k >= self.p.observes.len() {
                return Err(format!("a request's record {k} is not an observation"));
            }
        }
        for a in [rec.admitted, rec.first, rec.last_reused, rec.last_prefill] {
            self.attr(a)?;
        }
        Ok(())
    }

    /// An engine's connector: its load read at the request's admission, its
    /// save at its release; it loads into the engine's memory, which the
    /// request takes. Its blocks are checked with the session's.
    fn connector(&self, st: &CStep, c: &CConnector) -> Result<(), String> {
        let takes_memory = st.memory.is_some_and(|m| {
            st.request
                .iter()
                .flat_map(|r| &r.pools)
                .any(|(q, _, _)| q.base == m)
        });
        if let Some((e, b)) = &c.load {
            if !takes_memory {
                return Err(
                    "a connector loads into the engine's memory, which its requests do not take"
                        .into(),
                );
            }
            self.expr(e, Moment::Request)?;
            if e.any(&|x| matches!(x, CExpr::Ctx(CtxVar::Loaded))) {
                return Err(
                    "a connector's `load` says what `request.loaded` is: it reads `request.hit`, \
                     not itself"
                        .into(),
                );
            }
            if draws(e) {
                return Err("a connector's load may not draw".into());
            }
            self.block_id(*b)?;
        }
        if let Some((e, b)) = &c.save {
            if st.request.as_ref().is_none_or(|r| r.pools.is_empty()) {
                return Err(
                    "a connector saves before a request gives its pools back, and its \
                            requests take none"
                        .into(),
                );
            }
            self.expr(e, Moment::Release)?;
            self.block_id(*b)?;
        }
        if c.record >= self.p.observes.len() {
            return Err(format!(
                "a connector's record {} is not an observation",
                c.record
            ));
        }
        self.attr(c.loaded)?;
        self.attr(c.load_done)?;
        if let Some(a) = c.from {
            self.attr(a)?;
        }
        self.attr(c.saved)
    }

    fn block_id(&self, b: BlockId) -> Result<(), String> {
        if b >= self.p.blocks.len() {
            return Err(format!("block {b} out of range"));
        }
        Ok(())
    }

    fn stmt(&self, s: &CStmt) -> Result<(), String> {
        let np = self.p.pools.len();
        let ns = self.p.stages.len();
        let m = Moment::Session;
        match s {
            CStmt::Turn | CStmt::End => Ok(()),
            CStmt::Set(a, e) => {
                self.attr(*a)?;
                self.serial_unset(*a)?;
                self.expr(e, m)
            }
            CStmt::Observe(k, e) => {
                self.observe(*k)?;
                self.expr(e, m)
            }
            CStmt::Hold {
                pools,
                reuse,
                body,
                cache,
                prefix,
                lease,
            } => {
                // the header is read when the scheduler admits; `cache` when
                // the session releases
                // and re-read at every admission attempt, so a draw there
                // would be a different number each time the scheduler
                // looked (the rule of a queue key)
                let no_draw = |e: &CExpr, what: &str| {
                    if draws(e) {
                        Err(format!(
                            "a hold's {what} may not draw; sample into an attribute first"
                        ))
                    } else {
                        Ok(())
                    }
                };
                // a hold takes each pool once: two entries for one pool are
                // two allocations the statements inside cannot tell apart
                // (`grow kv` grew one and was given back twice, #309)
                for (r, _, _) in pools {
                    self.cref(r, np, "pool", m)?;
                }
                for (k, (r, _, _)) in pools.iter().enumerate() {
                    if pools[..k].iter().any(|(q, _, _)| q == r) {
                        return Err(format!(
                            "a hold takes `{}` twice; hold it once, with the units (and any \
                             reserve) added",
                            self.p.show_pool_ref(r)
                        ));
                    }
                }
                for (r, u, reserve) in pools {
                    self.expr(u, Moment::Admit)?;
                    no_draw(u, "units")?;
                    amount(u, &format!("hold {}", self.p.pools[r.base].name))?;
                    if let Some(f) = reserve {
                        self.expr(f, Moment::Admit)?;
                        no_draw(f, "`reserve`")?;
                    }
                    // what admission waits for, when the program fixes it,
                    // must fit some member the reference may name: one that
                    // fits none is rejected whenever it is reached (#271)
                    let need = [Some(u), reserve.as_ref()]
                        .into_iter()
                        .flatten()
                        .filter_map(constant)
                        .reduce(f64::max);
                    let round = |q: &CPool, x: f64| q.block.map_or(x, |b| (x / b).ceil() * b);
                    // the member a constant index names, or every member
                    let members = match r.index.as_deref().and_then(constant) {
                        Some(k) if k >= 0.0 && k.fract() == 0.0 && (k as usize) < r.count => {
                            r.base + k as usize..r.base + k as usize + 1
                        }
                        _ => r.base..r.base + r.count,
                    };
                    let several = members.len() > 1;
                    if let Some(need) = need
                        && self.p.pools[members.clone()]
                            .iter()
                            .all(|q| round(q, need) > q.cap)
                    {
                        let q = &self.p.pools[members.start];
                        let blocks = match q.block {
                            Some(b) if round(q, need) != need => {
                                format!(" ({} in blocks of {b})", round(q, need))
                            }
                            _ => String::new(),
                        };
                        let cap = if several {
                            format!("the cap of every member (`{}`: {})", q.name, q.cap)
                        } else {
                            format!("its cap {}", q.cap)
                        };
                        return Err(format!(
                            "a hold on pool `{}` waits for {need} units{blocks}, more than {cap}",
                            q.name
                        ));
                    }
                }
                if let Some(e) = reuse {
                    self.expr(e, Moment::Admit)?;
                    no_draw(e, "`reuse`")?;
                }
                if let Some(e) = cache {
                    self.expr(e, m)?;
                }
                // a key names what a segment holds: it is read while the
                // hold waits (a waiting order may read `cachedin`) and at its
                // admission, and the trie the keys index must not move under
                // them, so a key or a length reads no state, no clock, no
                // draw, and not the `cached` and `computed` admission sets
                for (k, n) in prefix.iter().flatten() {
                    for (e, what) in [(k, "key"), (n, "length")] {
                        self.expr(e, Moment::Admit)?;
                        if moves(e)
                            || e.any(&|x| {
                                matches!(x, CExpr::Attr(a)
                                    if *a == self.p.slot_cached || *a == self.p.slot_computed)
                            })
                        {
                            return Err(format!(
                                "a `prefix` segment's {what} reads the state, the clock, a draw, \
                                 or the `cached` and `computed` admission sets; a key names \
                                 content, so it is an attribute or a number, read the same while \
                                 the hold waits and when it is admitted"
                            ));
                        }
                    }
                }
                // a chain has a segment, and only its last runs on without
                // end: past an endless one no other is reached
                if let Some(segs) = prefix {
                    if segs.is_empty() {
                        return Err("an empty `prefix`: a chain has a segment".into());
                    }
                    if segs[..segs.len() - 1]
                        .iter()
                        .any(|(_, n)| matches!(n, CExpr::Num(x) if x.is_infinite()))
                    {
                        return Err(
                            "a `prefix` segment other than the last is endless (`inf`): \
                                    no segment after it would be reached"
                                .into(),
                        );
                    }
                }
                // one meaning, one spelling: the chain a hold without the
                // clause has is not written
                if let Some([(CExpr::Attr(a), CExpr::Num(n))]) = prefix.as_deref()
                    && *a == self.p.slot_serial
                    && *n == f64::INFINITY
                {
                    return Err(
                        "`prefix (serial: inf)` is the chain of a hold without `prefix`, \
                                the session's own; leave the clause out"
                            .into(),
                    );
                }
                // a chain is a sequence of blocks: a pool without them (a slot
                // pool) caches per session, and a hold with nothing else
                // would share nothing it says it does
                if prefix.is_some()
                    && !pools.iter().any(|(r, _, _)| {
                        self.p.pools[r.base..r.base + r.count]
                            .iter()
                            .all(|q| q.block.is_some())
                    })
                {
                    return Err(
                        "a hold with `prefix` takes no pool with `block`: a chain is \
                                cached in blocks, and a pool without them caches per session"
                            .into(),
                    );
                }
                // a spilled block goes to the tier under its session's
                // serial, which a chain of shared segments does not have
                if prefix.is_some()
                    && let Some((r, _, _)) = pools.iter().find(|(r, _, _)| {
                        self.p.pools[r.base..r.base + r.count]
                            .iter()
                            .enumerate()
                            .any(|(k, q)| {
                                q.spill.is_some()
                                    || self.p.pools.iter().any(|o| {
                                        o.spill.as_ref().is_some_and(|s| s.to == r.base + k)
                                    })
                            })
                    })
                {
                    return Err(format!(
                        "a hold with `prefix` on `{}`, which spills or is spilled to: a spilled \
                         block keeps its session's entry, not a chain of shared segments; not \
                         supported yet",
                        self.p.show_pool_ref(r)
                    ));
                }
                if let Some((r, t)) = lease {
                    self.cref(r, np, "pool", m)?;
                    self.expr(t, m)?;
                }
                self.block(*body)
            }
            CStmt::Grow(r, e) => {
                self.cref(r, np, "pool", m)?;
                self.expr(e, m)?;
                amount(e, &format!("grow {}", self.p.pools[r.base].name))
            }
            CStmt::Shrink(r, e) => {
                self.cref(r, np, "pool", m)?;
                self.expr(e, m)?;
                amount(e, &format!("shrink {}", self.p.pools[r.base].name))
            }
            CStmt::Drop(r) | CStmt::Release(r) => self.cref(r, np, "pool", m),
            CStmt::Load(r, e) => {
                self.cref(r, np, "pool", m)?;
                self.expr(e, m)?;
                amount(e, &format!("load {}", self.p.pools[r.base].name))
            }
            CStmt::Request {
                stage,
                input,
                output,
            } => {
                self.cref(stage, ns, "stage", m)?;
                let members = stage.base..stage.base + stage.count;
                if let Some(i) = members
                    .clone()
                    .find(|&i| !matches!(self.p.stages[i].kind, CStageKind::Step(_)))
                {
                    return Err(format!(
                        "a request is to an engine, and `{}` is a {}",
                        self.p.stages[i].name,
                        self.p.stages[i].kind.noun()
                    ));
                }
                let name = &self.p.stages[stage.base].name;
                for (e, what) in [(input, "input"), (output, "output")] {
                    self.expr(e, m)?;
                    amount(e, &format!("{name} {what}"))?;
                }
                // a request generates at least one token, as vLLM's
                // `max_tokens` is at least 1 (sampling_params.py:612-614)
                if constant(output).is_some_and(|o| o < 1.0) {
                    return Err(format!(
                        "a request to `{name}` generates {}: a request generates at least one \
                         token",
                        self.p.show_expr(output)
                    ));
                }
                Ok(())
            }
            CStmt::Run {
                stage,
                mode,
                work,
                growing,
                also,
                latency,
                ..
            } => {
                self.cref(stage, ns, "stage", m)?;
                let step = |i: usize| matches!(self.p.stages[i].kind, CStageKind::Step(_));
                let members = stage.base..stage.base + stage.count;
                if members
                    .clone()
                    .any(|i| step(i) != (*mode != RunMode::Plain))
                {
                    return Err(
                        "`prefill`/`decode` are required on an engine and not allowed on another stage"
                            .into(),
                    );
                }
                if growing.is_some() && !members.clone().all(step) {
                    return Err("`growing` needs an engine".into());
                }
                for r in also {
                    self.cref(r, ns, "stage", m)?;
                }
                self.expr(work, m)?;
                amount(work, &format!("run {}", self.p.stages[stage.base].name))?;
                if let Some(l) = latency {
                    if members.clone().any(step) {
                        return Err("a run's `latency` is a wait before a transfer, not before an engine's iteration".into());
                    }
                    self.expr(l, m)?;
                    amount(
                        l,
                        &format!("run {} latency", self.p.stages[stage.base].name),
                    )?;
                }
                if let Some(g) = growing {
                    self.cref(g, np, "pool", m)?;
                }
                Ok(())
            }
            CStmt::Branch(c, a, b) => {
                self.expr(c, m)?;
                self.block(*a)?;
                self.block(*b)
            }
            CStmt::While(c, b) => {
                self.expr(c, m)?;
                if let CExpr::Num(x) = c
                    && *x != 0.0
                    && *x != 1.0
                {
                    return Err(format!(
                        "`while ({})`: the guard is not 0 or 1; write `~bernoulli(p)` for a continuation probability",
                        show_num_exact(*x)
                    ));
                }
                self.block(*b)?;
                if !self.lets_time_pass(*b) {
                    return Err("a `while` must let time pass on every pass through its body: a `run`, a `hold` whose body does, or `end` on every path".into());
                }
                Ok(())
            }
            CStmt::Loop(b) => {
                self.block(*b)?;
                if !self.lets_time_pass(*b) {
                    return Err(
                        "a `loop` must let time pass on every pass through its body: \
                                a `run`, a `hold` whose body does, or `end` on every path"
                            .into(),
                    );
                }
                Ok(())
            }
            CStmt::Choose { var, count, key } => {
                self.attr(*var)?;
                self.serial_unset(*var)?;
                self.expr(count, m)?;
                if key.is_empty() {
                    return Err("a `choose` has no key".into());
                }
                key.iter().try_for_each(|k| self.expr(k, m))
            }
            CStmt::Fork(b) => {
                self.block(*b)?;
                if !self
                    .p
                    .blocks
                    .iter()
                    .flatten()
                    .any(|s| matches!(s, CStmt::Join))
                {
                    return Err(
                        "a program that forks a leg and never joins: a session may not \
                         end while its leg runs"
                            .into(),
                    );
                }
                if let Some(what) = self.leg_may_not(*b) {
                    return Err(format!(
                        "a `fork`'s leg may not {what}: a leg is a part of the request beside \
                         it, and the session turns, ends, forks and joins"
                    ));
                }
                Ok(())
            }
            CStmt::Join => {
                if self
                    .p
                    .blocks
                    .iter()
                    .flatten()
                    .any(|s| matches!(s, CStmt::Fork(_)))
                {
                    Ok(())
                } else {
                    Err("a `join` in a program that forks no leg waits for nothing".into())
                }
            }
        }
    }

    /// The first thing a leg's block does that only the session may: a
    /// `turn`, an `end`, a `fork` or a `join`, in it or a block in it.
    fn leg_may_not(&self, b: BlockId) -> Option<&'static str> {
        self.p.blocks[b].iter().find_map(|s| match s {
            CStmt::Turn => Some("`turn`"),
            CStmt::End => Some("`end`"),
            CStmt::Fork(_) => Some("fork"),
            CStmt::Join => Some("`join`"),
            CStmt::Hold { body, .. } | CStmt::Loop(body) | CStmt::While(_, body) => {
                self.leg_may_not(*body)
            }
            CStmt::Branch(_, x, y) => self.leg_may_not(*x).or_else(|| self.leg_may_not(*y)),
            _ => None,
        })
    }
}

// ---------------------------------------------------------------------------
// Printing expressions back to source form.
//
// The IR keeps no source text: names are slots and `let` constants are folded.
// Anything that shows a program to a person - a figure's label, an error that
// wants to quote the expression it is about - needs them back. The precedence
// levels below are those of `parser.rs`, so the output re-parses to the same
// tree (modulo the folded constants, which are numbers by then, and a
// fraction's digits past the twelfth, which `show_num` leaves off).
// ---------------------------------------------------------------------------

/// Precedence levels of `parser.rs`, lowest binding first.
mod prec {
    pub const COND: u8 = 0;
    pub const OR: u8 = 1;
    pub const AND: u8 = 2;
    pub const CMP: u8 = 3;
    pub const ADD: u8 = 4;
    pub const MUL: u8 = 5;
    pub const UNARY: u8 = 6;
    pub const POW: u8 = 7;
    pub const ATOM: u8 = 8;
}

impl BinOp {
    fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Pow => "^",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }

    fn precedence(self) -> u8 {
        match self {
            BinOp::Or => prec::OR,
            BinOp::And => prec::AND,
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne => prec::CMP,
            BinOp::Add | BinOp::Sub => prec::ADD,
            BinOp::Mul | BinOp::Div => prec::MUL,
            BinOp::Pow => prec::POW,
        }
    }
}

/// What a function takes in one argument position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgKind {
    Expr,
    Pool,
    Stage,
}

impl ArgKind {
    /// As the reference writes it in a signature: `queued(p: pool)`.
    pub fn name(self) -> &'static str {
        match self {
            ArgKind::Expr => "expr",
            ArgKind::Pool => "pool",
            ArgKind::Stage => "stage",
        }
    }
}

use ArgKind::{Expr as E, Pool as P, Stage as S};

/// A function's row of the table (`Fun::entry`).
struct FunEntry {
    name: &'static str,
    kinds: &'static [ArgKind],
    params: &'static [&'static str],
    group: FunGroup,
}

/// What a function reads: its arguments alone, or a stage's, a pool's or
/// an engine's state (`Fun::group`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunGroup {
    Arithmetic,
    Stage,
    Pool,
    Engine,
}

impl FunGroup {
    /// As the reference names its section: `arithmetic`, `stage`, ….
    pub fn name(self) -> &'static str {
        match self {
            FunGroup::Arithmetic => "arithmetic",
            FunGroup::Stage => "stage",
            FunGroup::Pool => "pool",
            FunGroup::Engine => "engine",
        }
    }
}

/// Every function a call may name, for lookup by name; `Fun::entry` is
/// the definition and a match, so a new `Fun` without one does not compile.
const FUNS: [Fun; 22] = [
    Fun::Min,
    Fun::Max,
    Fun::Abs,
    Fun::Floor,
    Fun::Ceil,
    Fun::Sqrt,
    Fun::Exp,
    Fun::Ln,
    Fun::Queue,
    Fun::Busy,
    Fun::Work,
    Fun::Used,
    Fun::Free,
    Fun::CachedIn,
    Fun::Allocated,
    Fun::Holders,
    Fun::Queued,
    Fun::Price,
    Fun::BudgetLeft,
    Fun::EstLambda,
    Fun::EstRho,
    Fun::EstWait,
];

impl Fun {
    /// A function of its arguments alone (`min`, `ceil`, …), which reads
    /// no state: its group says so (`Fun::entry`), not its signature, since
    /// a function of numbers alone may still read state.
    pub fn is_arithmetic(self) -> bool {
        self.group() == FunGroup::Arithmetic
    }

    /// The value of an arithmetic function, the one definition the
    /// interpreter and the constant folds share; `None` for a function that
    /// reads state, or for arguments of the wrong number.
    pub fn arithmetic(self, xs: &[f64]) -> Option<f64> {
        Some(match (self, xs) {
            (Fun::Min, [a, b]) => a.min(*b),
            (Fun::Max, [a, b]) => a.max(*b),
            (Fun::Abs, [a]) => a.abs(),
            (Fun::Floor, [a]) => a.floor(),
            (Fun::Ceil, [a]) => a.ceil(),
            (Fun::Sqrt, [a]) => a.sqrt(),
            (Fun::Exp, [a]) => a.exp(),
            (Fun::Ln, [a]) => a.ln(),
            _ => return None,
        })
    }

    /// The function's name in a program, the kind and name of each
    /// argument, and its group, whose `Arithmetic` is what `is_arithmetic`
    /// reads. The linker resolves a call by it, and `Program::validate`
    /// checks one; an editor reads it too (`frontend::highlight::functions`).
    fn entry(self) -> FunEntry {
        use FunGroup::{Arithmetic as A, Engine as En, Pool as Po, Stage as St};
        let f = |name, kinds, params, group| FunEntry {
            name,
            kinds,
            params,
            group,
        };
        match self {
            Fun::Min => f("min", &[E, E], &["a", "b"], A),
            Fun::Max => f("max", &[E, E], &["a", "b"], A),
            Fun::Abs => f("abs", &[E], &["x"], A),
            Fun::Floor => f("floor", &[E], &["x"], A),
            Fun::Ceil => f("ceil", &[E], &["x"], A),
            Fun::Sqrt => f("sqrt", &[E], &["x"], A),
            Fun::Exp => f("exp", &[E], &["x"], A),
            Fun::Ln => f("ln", &[E], &["x"], A),
            Fun::Queue => f("queue", &[S], &["s"], St),
            Fun::Busy => f("busy", &[S], &["s"], St),
            Fun::Work => f("work", &[S], &["s"], St),
            Fun::Used => f("used", &[P], &["p"], Po),
            Fun::Free => f("free", &[P], &["p"], Po),
            Fun::CachedIn => f("cachedin", &[P], &["p"], Po),
            Fun::Allocated => f("allocated", &[P], &["p"], Po),
            Fun::Holders => f("holders", &[P], &["p"], Po),
            Fun::Queued => f("queued", &[P], &["p"], Po),
            Fun::Price => f("price", &[S, E, E], &["s", "s_hit", "ds"], St),
            Fun::BudgetLeft => f("budget_left", &[S], &["s"], En),
            Fun::EstLambda => f("est_lambda", &[S], &["s"], St),
            Fun::EstRho => f("est_rho", &[S], &["s"], St),
            Fun::EstWait => f("est_wait", &[S], &["s"], St),
        }
    }

    pub fn name(self) -> &'static str {
        self.entry().name
    }

    /// The kind of each argument, in order.
    pub fn signature(self) -> &'static [ArgKind] {
        self.entry().kinds
    }

    /// The name of each parameter, in order, as the reference names it.
    pub fn params(self) -> &'static [&'static str] {
        self.entry().params
    }

    /// What the function reads: its arguments alone, or a stage's, a
    /// pool's or an engine's state; the reference's group.
    pub fn group(self) -> FunGroup {
        self.entry().group
    }

    /// Every function, in the table's order.
    pub fn all() -> impl Iterator<Item = Fun> {
        FUNS.into_iter()
    }

    pub fn from_name(name: &str) -> Option<Fun> {
        FUNS.into_iter().find(|f| f.name() == name)
    }

    /// The names a call may use, in the table's order.
    pub fn names() -> impl Iterator<Item = &'static str> {
        FUNS.into_iter().map(Fun::name)
    }
}

/// Every distribution a draw may name, for lookup by name; `DistKind::entry`
/// is the definition.
const DISTS: [DistKind; 6] = [
    DistKind::Exp,
    DistKind::Det,
    DistKind::Uniform,
    DistKind::Erlang,
    DistKind::H2,
    DistKind::Bernoulli,
];

impl DistKind {
    /// The distribution's name in a program and how many parameters it takes.
    fn entry(self) -> (&'static str, usize) {
        match self {
            DistKind::Exp => ("exp", 1),
            DistKind::Det => ("det", 1),
            DistKind::Uniform => ("uniform", 2),
            DistKind::Erlang => ("erlang", 2),
            DistKind::H2 => ("h2", 2),
            DistKind::Bernoulli => ("bernoulli", 1),
        }
    }

    pub fn name(self) -> &'static str {
        self.entry().0
    }

    pub fn arity(self) -> usize {
        self.entry().1
    }

    pub fn from_name(name: &str) -> Option<DistKind> {
        DISTS.into_iter().find(|d| d.name() == name)
    }

    /// The names a draw may use, in the table's order.
    pub fn names() -> impl Iterator<Item = &'static str> {
        DISTS.into_iter().map(DistKind::name)
    }
}

/// A number as a person would write it in a program: `16`, `2e-5`, `inf`.
///
/// Rust's `{}` writes `0.000000002` for `2e-9`, which is unreadable in a
/// label; `{:e}` writes `1.6e5` for `160000`, which is worse. Whole numbers
/// that fit take the plain form, the rest take whichever is shorter. A
/// fraction is shown to 12 significant digits: `0.1 * (1 + 8.1)` folds to
/// `0.9100000000000001`, which nobody wrote. Where the text is code to paste
/// back, `show_num_exact` keeps every digit.
pub fn show_num(x: f64) -> String {
    if x.is_finite() && x != x.trunc() {
        let r: f64 = format!("{x:.11e}").parse().unwrap_or(x);
        // a fraction the rounding would turn whole is shown as it is:
        // `w.p. 1` would read as certain, 999999999999.5 as an integer
        if r != r.trunc() {
            return show_num_exact(r);
        }
    }
    show_num_exact(x)
}

/// `show_num` without the rounding: the shortest text that reads back as `x`.
pub fn show_num_exact(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-inf".into() } else { "inf".into() };
    }
    if x == x.trunc() && x.abs() < 1e15 {
        return format!("{}", x as i64);
    }
    let plain = format!("{x}");
    let sci = format!("{x:e}");
    if sci.len() < plain.len() { sci } else { plain }
}

impl Program {
    /// Whether every `observe` of slot `k` records a test: an expression
    /// whose outermost operator is a comparison, `&&`, `||` or `!`, so 0 or
    /// 1 by construction (`observe hit = c > 0`). A literal `1` (an event
    /// counted), a time, and `c > 0 ? 1 : 0` are not, whatever values they
    /// take: the report notes a test that never held, and a miss there
    /// costs less than a note on a correct program.
    pub fn observe_is_test(&self, k: usize) -> bool {
        let mut seen = false;
        for block in &self.blocks {
            for s in block {
                if let CStmt::Observe(slot, e) = s
                    && *slot == k
                {
                    seen = true;
                    let test = match e {
                        CExpr::Binary(op, _, _) => matches!(
                            op,
                            BinOp::Lt
                                | BinOp::Le
                                | BinOp::Gt
                                | BinOp::Ge
                                | BinOp::Eq
                                | BinOp::Ne
                                | BinOp::And
                                | BinOp::Or
                        ),
                        CExpr::Unary(UnOp::Not, _) => true,
                        _ => false,
                    };
                    if !test {
                        return false;
                    }
                }
            }
        }
        seen
    }

    /// An expression in source form, with attribute, pool and stage names.
    ///
    /// `let` constants were folded at link time, so they come back as their
    /// values: `cap blocks * bs` prints as `160000`.
    pub fn show_expr(&self, e: &CExpr) -> String {
        self.show_expr_with(e, CtxVar::name)
    }

    /// An expression with its context variables named by `ctx`: a view
    /// that writes a step stage in an engine's words names the residents'
    /// totals by their list.
    pub fn show_expr_with(&self, e: &CExpr, ctx: CtxNames) -> String {
        let mut s = String::new();
        self.write_expr(&mut s, e, prec::COND, ctx);
        s
    }

    /// An `if` guard, as a figure should label it.
    ///
    /// A draw is written `if (~bernoulli(p))`, so it reaches here as a
    /// `Sample`. Rendering it `w.p. p` is what the lecture's
    /// own figure writes by hand ("resume w.p. p"), and it is the only thing
    /// that tells a reader this edge is a draw rather than a test.
    pub fn show_guard(&self, e: &CExpr) -> String {
        match e {
            CExpr::Sample(DistKind::Bernoulli, args) if args.len() == 1 => {
                format!("w.p. {}", self.show_expr(&args[0]))
            }
            _ => self.show_expr(e),
        }
    }

    /// The engines that have a connector, with it.
    pub fn connectors(&self) -> impl Iterator<Item = (&CStep, &CConnector)> {
        self.stages.iter().filter_map(|s| match &s.kind {
            CStageKind::Step(st) => st.connector.as_deref().map(|c| (st, c)),
            _ => None,
        })
    }

    /// A pool reference: `kv`, `rep[j]`.
    pub fn show_pool_ref(&self, r: &CRef) -> String {
        self.show_ref(r, |i| self.pools.get(i).map(|p| p.name.as_str()))
    }

    /// A stage reference: `engine`, `rep[j]`.
    pub fn show_stage_ref(&self, r: &CRef) -> String {
        self.show_ref(r, |i| self.stages.get(i).map(|s| s.name.as_str()))
    }

    fn show_ref<'a>(&self, r: &CRef, name: impl Fn(usize) -> Option<&'a str>) -> String {
        let base = name(r.base).unwrap_or("?").to_string();
        match &r.index {
            None => base,
            Some(i) => format!("{base}[{}]", self.show_expr(i)),
        }
    }

    fn attr_name(&self, slot: usize) -> &str {
        self.attrs.get(slot).map_or("?", String::as_str)
    }

    fn write_expr(&self, out: &mut String, e: &CExpr, min: u8, ctx: CtxNames) {
        use std::fmt::Write as _;
        match e {
            CExpr::Cost(target, value) => {
                let _ = write!(out, "cost({}, ", self.show_cost_target(target));
                self.write_expr(out, value, prec::COND, ctx);
                out.push(')');
            }
            CExpr::Num(x) => {
                // `pow()` parses `atom() '^' unary()`, so a folded negative
                // constant on the left of `^` has to be bracketed or the
                // output re-parses as `-(2 ^ a)`.
                if *x < 0.0 && min >= prec::UNARY {
                    let _ = write!(out, "({})", show_num(*x));
                } else {
                    out.push_str(&show_num(*x));
                }
            }
            CExpr::Attr(slot) => out.push_str(self.attr_name(*slot)),
            CExpr::Ctx(v) => out.push_str(ctx(*v)),
            CExpr::Sample(d, args) => {
                let _ = write!(out, "~{}(", d.name());
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    self.write_expr(out, a, prec::COND, ctx);
                }
                out.push(')');
            }
            CExpr::Call(f, args) => {
                let _ = write!(out, "{}(", f.name());
                self.write_list(out, args, ctx);
                out.push(')');
            }
            CExpr::Agg(a, k) => {
                let name = self.observes.get(*k).map_or("?", String::as_str);
                let _ = write!(out, "{}({name})", a.name());
            }
            CExpr::Reg(r) => {
                out.push_str(self.registers.get(*r).map_or("?", |g| g.name.as_str()));
            }
            CExpr::Unary(op, a) => {
                let wrap = min > prec::UNARY;
                if wrap {
                    out.push('(');
                }
                out.push_str(match op {
                    UnOp::Neg => "-",
                    UnOp::Not => "!",
                });
                self.write_expr(out, a, prec::UNARY, ctx);
                if wrap {
                    out.push(')');
                }
            }
            CExpr::Binary(op, a, b) => {
                let p = op.precedence();
                let wrap = min > p;
                if wrap {
                    out.push('(');
                }
                // `^` is right associative and takes an atom on the left;
                // every other operator is left associative.
                let (l, r) = if *op == BinOp::Pow {
                    (prec::ATOM, prec::UNARY)
                } else {
                    (p, p + 1)
                };
                self.write_expr(out, a, l, ctx);
                let _ = write!(out, " {} ", op.symbol());
                self.write_expr(out, b, r, ctx);
                if wrap {
                    out.push(')');
                }
            }
            CExpr::Cond(c, a, b) => {
                let wrap = min > prec::COND;
                if wrap {
                    out.push('(');
                }
                self.write_expr(out, c, prec::OR, ctx);
                out.push_str(" ? ");
                self.write_expr(out, a, prec::COND, ctx);
                out.push_str(" : ");
                self.write_expr(out, b, prec::COND, ctx);
                if wrap {
                    out.push(')');
                }
            }
        }
    }

    fn write_list(&self, out: &mut String, args: &[CArg], ctx: CtxNames) {
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            match a {
                CArg::Expr(e) => self.write_expr(out, e, prec::COND, ctx),
                CArg::Pool(r) => out.push_str(&self.show_pool_ref(r)),
                CArg::Stage(r) => out.push_str(&self.show_stage_ref(r)),
            }
        }
    }
}
