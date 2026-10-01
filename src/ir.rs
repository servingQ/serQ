//! The serQ intermediate representation (IR).
//!
//! The IR is the definition of a serQ program: the interpreter (`engine::interp`) runs
//! it, the Lean model is generated from it, and tools build or edit it as
//! data. The text syntax (`frontend`) is one frontend that compiles
//! to it. The format is serialised as JSON (`Program::to_json`), carries a
//! version, and is checked on load (`Program::validate`). See `docs/ir.md`.

pub mod trace;

use serde::{Deserialize, Serialize};

/// Version of the IR format. Bump on any change to the types below.
/// 2 added the sessions' turns; 3 renamed `route` to `session`; 4 replaced
/// `CStep`'s `exclusive_prefill` and `decode_first` by `serve`; 5 added
/// `Release` and `Load`; 6 added renewal arrivals and finite open runs; 7
/// makes `Choose.key` a list of keys, compared in order; 8 lets a `Run`
/// hold several stages at once (`also`) under the program's `share` and
/// makes `ExclusivePrefill` isolate the whole batch, including admissions;
/// 9 reevaluates lexicographic queue keys at selection and supplies `Waited`;
/// 10 makes `Hold.cache` the clause that admits a hold to the prefix cache
/// (a hold without it consumes nothing of the session's own entry).
pub const IR_VERSION: u32 = 10;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preempt {
    /// A failed growth waits.
    None,
    /// A failed growth preempts the most recently admitted holder (vLLM).
    Lifo,
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
    /// A step stage's `serve by` keys: evaluated for one resident.
    Serve,
    /// A `gauge`: evaluated on the state the deployment holds after every
    /// instant, with no session, job or resident, and held until the next
    /// one, so neither `now` nor `work(…)`, which move in between, nor
    /// `budget_left(…)`, which plans an iteration to answer.
    Gauge,
}

impl std::fmt::Display for Moment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Moment::Session => "a session statement, a run or a hold's cache",
            Moment::Admit => "a hold's header, read at admission",
            Moment::Select => "a pool's queue keys, read before selecting a waiting session",
            Moment::Evict => "an eviction key or spill clause",
            Moment::Ps => "a ps stage's capacity",
            Moment::Budget => "a step stage's budget or chunk, planned before the iteration",
            Moment::Step => "a step stage's cost, after the iteration",
            Moment::Serve => "a step stage's serve keys",
            Moment::Gauge => "a gauge, read on the deployment's state with no session",
        })
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
}

impl CtxVar {
    /// The source spelling (`link.rs` maps the same names).
    fn name(self) -> &'static str {
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
            CtxVar::Nres | CtxVar::Ndec | CtxVar::Kvb | CtxVar::Kvp => {
                &[Moment::Budget, Moment::Step, Moment::Serve]
            }
            CtxVar::Ntok | CtxVar::Npre | CtxVar::Attn => &[Moment::Step],
            CtxVar::Decoding | CtxVar::Admission | CtxVar::Remaining => &[Moment::Serve],
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
    Pow,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CExpr {
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
        /// The pool (one of `pools`) whose allocation outlives the scope as
        /// a lease, and for how long at most: it stays allocated, neither
        /// evictable nor a preemption victim, until the session's `Release`
        /// of it, the expiry, or the session's end; then `cache` applies.
        /// vLLM's prefiller keeps a finished request's blocks this way for
        /// the decoder's read (`delay_free_blocks`).
        lease: Option<(CRef, CExpr)>,
    },
    Grow(CRef, CExpr),
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
        /// flow over `stage` and these, its rate set by the program's
        /// `share` from their capacities. Empty for a run on one stage.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        also: Vec<CRef>,
    },
    Branch(CExpr, BlockId, BlockId),
    Loop(BlockId),
    Choose {
        var: usize,
        count: CExpr,
        key: Vec<CExpr>,
    },
    End,
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
    pub spill: Option<CSpill>,
    pub admit_via: Option<usize>,
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
    /// How the iteration serves its residents: an order, or the
    /// exclusive-prefill rule.
    pub serve: CServe,
    pub memory: Option<usize>,
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
    Delay,
    Step(CStep),
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
    /// How the flows of runs over several stages divide the stages'
    /// capacity; present exactly when some `Run` has a non-empty `also`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<Share>,
    /// `gauge NAME = e;`: functions of the deployment's state whose time
    /// average the report gives. They read and do not act, so a reader that
    /// ignores them runs the same program (`docs/ir.md`, Stability).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gauges: Vec<Gauge>,
}

/// A gauge: a name and an expression evaluated at `Moment::Gauge`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gauge {
    pub name: String,
    pub expr: CExpr,
}

/// The sharing policy of `share`: how flows that hold several `ps` stages at
/// once divide their capacities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Share {
    /// Max-min fair: every flow's rate rises together until a stage fills,
    /// the flows through it stop there, and the rest go on.
    MaxMin,
    /// Each flow gets its equal share at the tightest of its stages,
    /// `min over s of φ_s / n_s`; what that leaves at the others is unused.
    Bottleneck,
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
    /// The stages some run over several stages holds, each array whole: a
    /// run's index is known only when it starts. Every job on one of them
    /// is a flow of the program's `share`; every other stage serves as its
    /// kind says.
    pub fn shared_stages(&self) -> Vec<bool> {
        let mut shared = vec![false; self.stages.len()];
        for b in &self.blocks {
            for st in b {
                if let CStmt::Run { stage, also, .. } = st
                    && !also.is_empty()
                {
                    for r in std::iter::once(stage).chain(also) {
                        for k in r.base..(r.base + r.count.max(1)).min(shared.len()) {
                            shared[k] = true;
                        }
                    }
                }
            }
        }
        shared
    }

    /// A run over several stages holds `ps` stages of a constant capacity
    /// (a flow is not described by the `n` a capacity could read), each
    /// stage array once (an index is known only when the run starts), and
    /// the program names its `share`; every run on a shared stage is plain.
    fn validate_flows(&self) -> Result<(), String> {
        let shared = self.shared_stages();
        let any = shared.iter().any(|&x| x);
        match (any, self.share) {
            (true, None) => {
                return Err(
                    "a run over several stages needs the program's `share` (`share maxmin;` or `share bottleneck;`)"
                        .into(),
                );
            }
            (false, Some(_)) => {
                return Err("`share` without a run over several stages".into());
            }
            _ => {}
        }
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
                    ..
                } = st
                else {
                    continue;
                };
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
        if self.version != IR_VERSION {
            return Err(format!(
                "IR version {} (this interpreter reads {IR_VERSION})",
                self.version
            ));
        }
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
            for s in b {
                v.stmt(s).map_err(|e| format!("{role}: {e}"))?;
            }
        }
        for (k, blk) in [
            ("init", self.init),
            ("turn", self.turn),
            ("session", self.session),
        ] {
            v.block(blk).map_err(|e| format!("{k}: {e}"))?;
        }
        self.validate_flows()?;
        for p in &self.pools {
            let at = |e| format!("pool `{}`: {e}", p.name);
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
            }
        }
        for s in &self.stages {
            let at = |e| format!("stage `{}`: {e}", s.name);
            match &s.kind {
                CStageKind::Fifo(_) | CStageKind::Delay => {}
                CStageKind::Ps(e) => v.expr(e, Moment::Ps).map_err(at)?,
                CStageKind::Step(st) => {
                    v.expr(&st.budget, Moment::Budget).map_err(at)?;
                    v.expr(&st.chunk, Moment::Budget).map_err(at)?;
                    v.expr(&st.cost, Moment::Step).map_err(at)?;
                    if let CServe::By(keys) = &st.serve {
                        for k in keys {
                            v.expr(k, Moment::Serve).map_err(at)?;
                            if draws(k) {
                                return Err(at(
                                    "a serve key may not draw (`~`): it is read for every \
                                     resident at every iteration, and the order would change \
                                     under the scheduler's feet"
                                        .into(),
                                ));
                            }
                        }
                    }
                    if let Some(m) = st.memory {
                        v.pool(m)?;
                    }
                }
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
        if let CArrival::Renewal(e) = &self.arrival {
            v.expr(e, Moment::Session)?;
            if !arrival_expr_is_pure(e) {
                return Err(
                    "renewal arrival may use only constants and sampled distributions".into(),
                );
            }
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

/// Whether an expression samples a distribution anywhere.
fn draws(e: &CExpr) -> bool {
    match e {
        CExpr::Sample(..) => true,
        CExpr::Num(_) | CExpr::Attr(_) | CExpr::Ctx(_) => false,
        CExpr::Call(_, args) => args.iter().any(|a| match a {
            CArg::Expr(x) => draws(x),
            CArg::Pool(r) | CArg::Stage(r) => r.index.as_ref().is_some_and(|i| draws(i)),
        }),
        CExpr::Unary(_, x) => draws(x),
        CExpr::Binary(_, a, b) => draws(a) || draws(b),
        CExpr::Cond(c, a, b) => draws(c) || draws(a) || draws(b),
    }
}

fn arrival_expr_is_pure(e: &CExpr) -> bool {
    match e {
        CExpr::Num(_) => true,
        CExpr::Sample(_, args) => args.iter().all(arrival_expr_is_pure),
        CExpr::Unary(_, x) => arrival_expr_is_pure(x),
        CExpr::Binary(_, a, b) => arrival_expr_is_pure(a) && arrival_expr_is_pure(b),
        CExpr::Cond(c, a, b) => {
            arrival_expr_is_pure(c) && arrival_expr_is_pure(a) && arrival_expr_is_pure(b)
        }
        CExpr::Call(_, args) => args.iter().all(|arg| match arg {
            CArg::Expr(x) => arrival_expr_is_pure(x),
            CArg::Pool(_) | CArg::Stage(_) => false,
        }),
        CExpr::Attr(_) | CExpr::Ctx(_) => false,
    }
}

struct Validator<'a> {
    p: &'a Program,
}

impl Validator<'_> {
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
        // a gauge names its members by number (an aggregate writes them so):
        // an index read from the state could leave the array, and a gauge
        // that fails the run would be one a reader ignoring gauges does not
        if m == Moment::Gauge
            && let Some(e) = &r.index
            && !matches!(**e, CExpr::Num(k) if k >= 0.0 && k.fract() == 0.0 && k < r.count as f64)
        {
            return Err(format!(
                "a gauge's {what} index is a number in range (`kv[0]`, or an aggregate's `kv[k]`), \
                 not one read from the state"
            ));
        }
        if let Some(e) = &r.index {
            self.expr(e, m)?;
        }
        Ok(())
    }
    /// The expression is well formed and reads only what moment `m` supplies.
    fn expr(&self, e: &CExpr, m: Moment) -> Result<(), String> {
        match e {
            CExpr::Num(_) => Ok(()),
            // a gauge is integrated as constant between events: what moves
            // between them would be read at the event and held
            CExpr::Ctx(CtxVar::Now) if m == Moment::Gauge => Err(
                "a gauge may not read `now`: it changes between events, and a gauge is held \
                 constant between them"
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
                 evaluates the stage's budget and may draw"
                    .into(),
            ),
            CExpr::Call(Fun::CachedIn, _) if m == Moment::Gauge => Err(
                "`cachedin` is the session's own cached prefix, and a gauge has no session".into(),
            ),
            CExpr::Attr(a) => {
                self.attr(*a)?;
                if m != Moment::Session && self.p.hidden.contains(a) {
                    return Err(format!(
                        "`{}` is hidden from the scheduler, but is read in {m}",
                        self.p.attrs[*a]
                    ));
                }
                Ok(())
            }
            CExpr::Sample(_, xs) => xs.iter().try_for_each(|x| self.expr(x, m)),
            CExpr::Call(_, args) => args.iter().try_for_each(|a| match a {
                CArg::Expr(x) => self.expr(x, m),
                CArg::Pool(r) => self.cref(r, self.p.pools.len(), "pool", m),
                CArg::Stage(r) => self.cref(r, self.p.stages.len(), "stage", m),
            }),
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
    fn stmt(&self, s: &CStmt) -> Result<(), String> {
        let np = self.p.pools.len();
        let ns = self.p.stages.len();
        let m = Moment::Session;
        match s {
            CStmt::Turn | CStmt::End => Ok(()),
            CStmt::Set(a, e) => {
                self.attr(*a)?;
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
                lease,
            } => {
                // the header is read when the scheduler admits; `cache` when
                // the session releases
                for (r, u, reserve) in pools {
                    self.cref(r, np, "pool", m)?;
                    self.expr(u, Moment::Admit)?;
                    if let Some(f) = reserve {
                        self.expr(f, Moment::Admit)?;
                    }
                }
                if let Some(e) = reuse {
                    self.expr(e, Moment::Admit)?;
                }
                if let Some(e) = cache {
                    self.expr(e, m)?;
                }
                if let Some((r, t)) = lease {
                    self.cref(r, np, "pool", m)?;
                    self.expr(t, m)?;
                }
                self.block(*body)
            }
            CStmt::Grow(r, e) => {
                self.cref(r, np, "pool", m)?;
                self.expr(e, m)
            }
            CStmt::Drop(r) | CStmt::Release(r) => self.cref(r, np, "pool", m),
            CStmt::Load(r, e) => {
                self.cref(r, np, "pool", m)?;
                self.expr(e, m)
            }
            CStmt::Run {
                stage,
                work,
                growing,
                also,
                ..
            } => {
                self.cref(stage, ns, "stage", m)?;
                for r in also {
                    self.cref(r, ns, "stage", m)?;
                }
                self.expr(work, m)?;
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
            CStmt::Loop(b) => self.block(*b),
            CStmt::Choose { var, count, key } => {
                self.attr(*var)?;
                self.expr(count, m)?;
                if key.is_empty() {
                    return Err("a `choose` has no key".into());
                }
                key.iter().try_for_each(|k| self.expr(k, m))
            }
        }
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

impl Fun {
    fn name(self) -> &'static str {
        match self {
            Fun::Min => "min",
            Fun::Max => "max",
            Fun::Abs => "abs",
            Fun::Floor => "floor",
            Fun::Ceil => "ceil",
            Fun::Sqrt => "sqrt",
            Fun::Exp => "exp",
            Fun::Ln => "ln",
            Fun::Pow => "pow",
            Fun::Queue => "queue",
            Fun::Busy => "busy",
            Fun::Work => "work",
            Fun::Used => "used",
            Fun::Free => "free",
            Fun::CachedIn => "cachedin",
            Fun::Holders => "holders",
            Fun::Queued => "queued",
            Fun::Price => "price",
            Fun::BudgetLeft => "budget_left",
            Fun::EstLambda => "est_lambda",
            Fun::EstRho => "est_rho",
            Fun::EstWait => "est_wait",
        }
    }
}

impl DistKind {
    fn name(self) -> &'static str {
        match self {
            DistKind::Exp => "exp",
            DistKind::Det => "det",
            DistKind::Uniform => "uniform",
            DistKind::Erlang => "erlang",
            DistKind::H2 => "h2",
            DistKind::Bernoulli => "bernoulli",
        }
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
        let mut s = String::new();
        self.write_expr(&mut s, e, prec::COND);
        s
    }

    /// A `branch` guard, as a figure should label it.
    ///
    /// `branch with (p)` is sugar for `branch (~bernoulli(p))`, so the draw
    /// reaches here as a `Sample`. Rendering it `w.p. p` is what the lecture's
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

    fn write_expr(&self, out: &mut String, e: &CExpr, min: u8) {
        use std::fmt::Write as _;
        match e {
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
            CExpr::Ctx(v) => out.push_str(v.name()),
            CExpr::Sample(d, args) => {
                let _ = write!(out, "~{}(", d.name());
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    self.write_expr(out, a, prec::COND);
                }
                out.push(')');
            }
            CExpr::Call(f, args) => {
                let _ = write!(out, "{}(", f.name());
                self.write_list(out, args);
                out.push(')');
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
                self.write_expr(out, a, prec::UNARY);
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
                self.write_expr(out, a, l);
                let _ = write!(out, " {} ", op.symbol());
                self.write_expr(out, b, r);
                if wrap {
                    out.push(')');
                }
            }
            CExpr::Cond(c, a, b) => {
                let wrap = min > prec::COND;
                if wrap {
                    out.push('(');
                }
                self.write_expr(out, c, prec::OR);
                out.push_str(" ? ");
                self.write_expr(out, a, prec::COND);
                out.push_str(" : ");
                self.write_expr(out, b, prec::COND);
                if wrap {
                    out.push(')');
                }
            }
        }
    }

    fn write_list(&self, out: &mut String, args: &[CArg]) {
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            match a {
                CArg::Expr(e) => self.write_expr(out, e, prec::COND),
                CArg::Pool(r) => out.push_str(&self.show_pool_ref(r)),
                CArg::Stage(r) => out.push_str(&self.show_stage_ref(r)),
            }
        }
    }
}
