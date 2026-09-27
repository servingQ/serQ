//! The seQ intermediate representation (IR).
//!
//! The IR is the definition of a seQ program: the interpreter (`sim`) runs
//! it, the Lean model is generated from it, and tools build or edit it as
//! data. The text syntax (`parser` + `link`) is one frontend that compiles
//! to it. The format is serialised as JSON (`Program::to_json`), carries a
//! version, and is checked on load (`Program::validate`). See `docs/ir.md`.

use serde::{Deserialize, Serialize};

/// Version of the IR format. Bump on any change to the types below.
pub const IR_VERSION: u32 = 2;

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

/// Context variables: meaningful only where the semantics supplies them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CtxVar {
    /// Simulation clock.
    Now,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CRef {
    pub base: usize,
    pub count: usize,
    pub index: Option<Box<CExpr>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CArg {
    Expr(CExpr),
    Pool(CRef),
    Stage(CRef),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CExpr {
    Num(f64),
    Attr(usize),
    Ctx(CtxVar),
    Sample(DistKind, Vec<CExpr>),
    Call(Fun, Vec<CArg>),
    Unary(UnOp, Box<CExpr>),
    Binary(BinOp, Box<CExpr>, Box<CExpr>),
    Cond(Box<CExpr>, Box<CExpr>, Box<CExpr>),
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
    },
    Grow(CRef, CExpr),
    Drop(CRef),
    Run {
        stage: CRef,
        mode: RunMode,
        work: CExpr,
        growing: Option<CRef>,
    },
    Branch(CExpr, BlockId, BlockId),
    Loop(BlockId),
    Choose {
        var: usize,
        count: CExpr,
        key: CExpr,
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
    pub cap: f64,
    pub block: Option<f64>,
    pub evict: CEvict,
    pub preempt: Preempt,
    pub queue: Option<CExpr>,
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
    pub exclusive_prefill: bool,
    pub decode_first: bool,
    pub memory: Option<usize>,
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
    pub kind: CStageKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CArrival {
    Poisson(f64),
    Closed(usize),
    Batch(usize),
    /// Explicit sessions, all arriving at time 0: each one's attributes
    /// are preset after its `init` block runs. The workload instance of a
    /// scenario (e.g. the vLLM oracle's requests) as data rather than as
    /// program text.
    Sessions(Vec<SessionInit>),
    None,
}

/// A seQ program in IR form: the deployment (pools, stages), the workload
/// (arrival, `init`/`turn` blocks, trace, or explicit sessions), the route
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
    pub route: BlockId,
    pub blocks: Vec<Vec<CStmt>>,
    pub horizon: f64,
    pub warmup: f64,
    pub seed: u64,
    /// Slots of the built-in attributes.
    pub slot_cached: usize,
    pub slot_serial: usize,
    pub slot_turn: usize,
    pub slot_new: usize,
    pub slot_out: usize,
    pub slot_think: usize,
    pub slot_more: usize,
    pub slot_forced: usize,
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
    /// JSON form of the IR.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("IR serialises")
    }

    /// Read and validate an IR from JSON.
    pub fn from_json(s: &str) -> Result<Program, String> {
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
    /// times are the route's business, e.g. a delay of `serial * spacing`).
    pub fn inline_trace(mut self, corpus: &crate::trace::Corpus) -> Result<Program, String> {
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

    /// Structural checks: every index refers to something that exists.
    /// The text frontend produces valid IR; this guards IR read from JSON
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
            for s in b {
                v.stmt(s).map_err(|e| format!("block {i}: {e}"))?;
            }
        }
        for (k, blk) in [
            ("init", self.init),
            ("turn", self.turn),
            ("route", self.route),
        ] {
            v.block(blk).map_err(|e| format!("{k}: {e}"))?;
        }
        for p in &self.pools {
            if let Some(e) = &p.queue {
                v.expr(e)?;
            }
            if let CEvict::By(keys) = &p.evict {
                for e in keys {
                    v.expr(e)?;
                }
            }
            if let Some(s) = &p.spill {
                v.pool(s.to)?;
                v.stage(s.via)?;
                v.expr(&s.work)?;
                v.expr(&s.when)?;
            }
            if let Some(s) = p.admit_via {
                v.stage(s)?;
            }
        }
        for s in &self.stages {
            match &s.kind {
                CStageKind::Fifo(_) | CStageKind::Delay => {}
                CStageKind::Ps(e) => v.expr(e)?,
                CStageKind::Step(st) => {
                    v.expr(&st.budget)?;
                    v.expr(&st.cost)?;
                    v.expr(&st.chunk)?;
                    if let Some(m) = st.memory {
                        v.pool(m)?;
                    }
                }
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
        ] {
            v.attr(slot)?;
        }
        if !(self.horizon.is_finite() && self.warmup >= 0.0 && self.warmup < self.horizon) {
            return Err("run: need 0 <= warmup < horizon".into());
        }
        Ok(())
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
    fn cref(&self, r: &CRef, n: usize, what: &str) -> Result<(), String> {
        if r.count == 0 || r.base + r.count > n {
            return Err(format!(
                "{what} reference {}..{} out of range",
                r.base,
                r.base + r.count
            ));
        }
        if let Some(e) = &r.index {
            self.expr(e)?;
        }
        Ok(())
    }
    fn expr(&self, e: &CExpr) -> Result<(), String> {
        match e {
            CExpr::Num(_) | CExpr::Ctx(_) => Ok(()),
            CExpr::Attr(a) => self.attr(*a),
            CExpr::Sample(_, xs) => xs.iter().try_for_each(|x| self.expr(x)),
            CExpr::Call(_, args) => args.iter().try_for_each(|a| match a {
                CArg::Expr(x) => self.expr(x),
                CArg::Pool(r) => self.cref(r, self.p.pools.len(), "pool"),
                CArg::Stage(r) => self.cref(r, self.p.stages.len(), "stage"),
            }),
            CExpr::Unary(_, x) => self.expr(x),
            CExpr::Binary(_, a, b) => {
                self.expr(a)?;
                self.expr(b)
            }
            CExpr::Cond(c, a, b) => {
                self.expr(c)?;
                self.expr(a)?;
                self.expr(b)
            }
        }
    }
    fn stmt(&self, s: &CStmt) -> Result<(), String> {
        let np = self.p.pools.len();
        let ns = self.p.stages.len();
        match s {
            CStmt::Turn | CStmt::End => Ok(()),
            CStmt::Set(a, e) => {
                self.attr(*a)?;
                self.expr(e)
            }
            CStmt::Observe(k, e) => {
                self.observe(*k)?;
                self.expr(e)
            }
            CStmt::Hold {
                pools,
                reuse,
                body,
                cache,
            } => {
                for (r, u, fits) in pools {
                    self.cref(r, np, "pool")?;
                    self.expr(u)?;
                    if let Some(f) = fits {
                        self.expr(f)?;
                    }
                }
                if let Some(e) = reuse {
                    self.expr(e)?;
                }
                if let Some(e) = cache {
                    self.expr(e)?;
                }
                self.block(*body)
            }
            CStmt::Grow(r, e) => {
                self.cref(r, np, "pool")?;
                self.expr(e)
            }
            CStmt::Drop(r) => self.cref(r, np, "pool"),
            CStmt::Run {
                stage,
                work,
                growing,
                ..
            } => {
                self.cref(stage, ns, "stage")?;
                self.expr(work)?;
                if let Some(g) = growing {
                    self.cref(g, np, "pool")?;
                }
                Ok(())
            }
            CStmt::Branch(c, a, b) => {
                self.expr(c)?;
                self.block(*a)?;
                self.block(*b)
            }
            CStmt::Loop(b) => self.block(*b),
            CStmt::Choose { var, count, key } => {
                self.attr(*var)?;
                self.expr(count)?;
                self.expr(key)
            }
        }
    }
}
