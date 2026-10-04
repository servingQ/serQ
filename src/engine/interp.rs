//! The serQ interpreter: a discrete-event simulator whose state is the
//! configuration of `docs/language.md`.
//!
//! Commands (the statements of a session) take no time and run whenever a
//! session is *ready*; flow lets time pass at the stages. After every
//! event the interpreter settles: it executes every ready session, retries
//! growers and admissions at every pool until nothing changes, then starts
//! an iteration on every idle step stage that has residents.

use std::cmp::Ordering;
use std::collections::{BTreeSet, BinaryHeap, HashMap, VecDeque};

use rand::Rng;
use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::engine::dist::Dist;
use crate::engine::report::*;
use crate::engine::stats::*;

/// The tolerance of a comparison of amounts that sums of floats produce
/// (units, tokens, work): below it, two amounts are equal.
const EPS: f64 = 1e-9;
use crate::frontend::ast::{BinOp, Preempt, RunMode, UnOp};
use crate::frontend::link::*;
use crate::ir::trace::Corpus;

// ------------------------------------------------------------ events ----

enum Ev {
    Arrive,
    Finish {
        stage: usize,
        job: u64,
        epoch: u64,
    },
    IterEnd {
        stage: usize,
        epoch: u64,
    },
    /// A lease's time is up: `serial` guards against a reused session slot.
    LeaseEnd {
        sid: usize,
        serial: u64,
        id: u64,
    },
    EndWarmup,
}

struct Entry {
    time: f64,
    seq: u64,
    ev: Ev,
}

impl PartialEq for Entry {
    fn eq(&self, o: &Self) -> bool {
        self.seq == o.seq
    }
}
impl Eq for Entry {}
impl Ord for Entry {
    fn cmp(&self, o: &Self) -> Ordering {
        o.time
            .total_cmp(&self.time)
            .then_with(|| o.seq.cmp(&self.seq))
    }
}
impl PartialOrd for Entry {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

// ---------------------------------------------------------- sessions ----

#[derive(Clone, Copy, Debug, PartialEq)]
enum FrameKind {
    Plain,
    Loop,
    Hold,
}

#[derive(Clone, Debug)]
struct Frame {
    block: BlockId,
    pc: usize,
    kind: FrameKind,
}

#[derive(Clone, Debug, PartialEq)]
enum Status {
    Ready,
    /// Waiting in the queue of a pool for a hold.
    Queued(usize),
    /// A job at a stage.
    InStage(usize, u64),
    /// A failed `grow` waiting for room (pool policy `none`); resumes at
    /// the stage job it was in, if any.
    Growing(usize, f64, Option<(usize, u64)>),
    Ended,
}

#[derive(Clone, Debug)]
struct Hold<'p> {
    /// (pool, allocated units)
    pools: Vec<(usize, f64)>,
    /// Per pool: the session's position in its sequence, advanced by
    /// `growing` runs; starts at the consumed cached prefix.
    pos: Vec<f64>,
    /// Whether a `growing` run advanced `pos` (then `pos`, not the
    /// allocation, is what has been computed).
    grown: bool,
    cache: Option<&'p CExpr>,
    /// The pool whose allocation outlives the scope, and for how long.
    lease: Option<(usize, &'p CExpr)>,
    body: BlockId,
}

/// An allocation that outlived its scope (`lease P (t)`): the session's
/// until its `release` of the pool, the expiry or its end, then cached per
/// `cache`. Not a hold: no `grow`, no `growing`, no preemption.
#[derive(Clone, Debug)]
struct Lease<'p> {
    id: u64,
    pool: usize,
    alloc: f64,
    computed: f64,
    cache: Option<&'p CExpr>,
}

#[derive(Clone, Debug)]
struct Pending<'p> {
    pools: Vec<(usize, f64)>,
    /// The unit expressions, re-evaluated at admission (the lecture's
    /// `[Admit]` evaluates `c(x_r)` then: observables such as the cache or
    /// the engine's budget may have changed while the session waited).
    exprs: Vec<&'p CExpr>,
    /// Per pool: units that must fit for the admission (default: the
    /// allocation), e.g. the whole prompt while only its first chunk is
    /// allocated (vLLM `scheduler_reserve_full_isl`).
    reserve: Vec<Option<&'p CExpr>>,
    need: Vec<f64>,
    reuse: Option<&'p CExpr>,
    cache: Option<&'p CExpr>,
    lease: Option<(usize, &'p CExpr)>,
    body: BlockId,
    queued_at: f64,
}

#[derive(Clone, Debug)]
struct Session<'p> {
    serial: u64,
    attrs: Vec<f64>,
    frames: Vec<Frame>,
    status: Status,
    holds: Vec<Hold<'p>>,
    pending: Option<Pending<'p>>,
    trace: Option<(usize, usize)>,
    /// (index in `CArrival::Sessions`, next turn) of a session with explicit turns
    script: Option<(usize, usize)>,
    /// Order of the session's latest hold admission: residents of a step
    /// stage are served in this order (vLLM's `running` list is in order of
    /// admission; a preempted request re-enters at the end).
    adm_seq: u64,
    /// Per pool, the position the session's hold had reached on that pool
    /// when it was last preempted for it; cleared when a hold completes. A
    /// second preemption for the same pool at the same or a lower position
    /// is no progress there: the session is `stuck`.
    preempt_pos: HashMap<usize, f64>,
    stuck: bool,
    /// Allocations that outlived their scope (`lease`).
    leases: Vec<Lease<'p>>,
    /// The request's latest token on a step stage, for the gaps between
    /// tokens (`Interp::token`).
    last_token: Option<LastToken>,
    /// The session's own random streams (`docs/design/stochastic-model.md`,
    /// Definition 3): `rng_wl` serves the workload's `init` and `turn`
    /// blocks and is reseeded from (seed, serial, turn) at every turn, so a
    /// (session, turn) draws the same marks whatever the rest of the
    /// deployment does; `rng` serves the session's own statements in
    /// program order. A draw the machine makes (a cost, a budget, an
    /// eviction key) reads the interpreter's streams, not a session's.
    rng_wl: StdRng,
    rng: StdRng,
    /// Turns taken, the key of `rng_wl`'s reseeding: the interpreter's
    /// count, not the attribute `turn_no`, which a program may overwrite.
    turn_count: u64,
    /// (instant, times the session became ready at it): a session that
    /// keeps becoming ready without time passing is a loop that never
    /// blocks (`READIES_PER_INSTANT`).
    readies: (f64, u64),
}

/// The stream of one (seed, session, turn, kind): splitmix64 over the four,
/// so that no two sessions, turns or kinds share a stream.
fn substream(seed: u64, serial: u64, turn: u64, kind: u64) -> StdRng {
    let mut x = seed;
    for v in [serial, turn, kind] {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15).wrapping_add(v);
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^= x >> 31;
    }
    StdRng::seed_from_u64(x)
}

/// Kinds of a session's streams (`substream`).
const STREAM_WORKLOAD: u64 = 1;
const STREAM_SESSION: u64 = 2;

/// Statements a session may execute without blocking before the run is an
/// error: a loop that never reaches a `run`, a `hold` that waits, or `end`
/// would otherwise hang the interpreter (`docs/design/stochastic-model.md`,
/// Lemma 1).
const STEPS_PER_INSTANT: u64 = 1_000_000;

/// Times one session may become ready at one instant: a run of zero work
/// or a hold admitted and released inside a loop re-readies its session
/// without time passing. Per session, so that an instant with many
/// sessions (a `batch` arrival) is not mistaken for one.
const READIES_PER_INSTANT: u64 = 1_000;

#[derive(Clone, Copy, Debug)]
struct LastToken {
    turn: f64,
    at: f64,
    /// The stage it was committed on.
    stage: usize,
    /// Whether the turn has decoded a token.
    decoded: bool,
    /// Whether the request has been preempted since this token.
    preempted: bool,
}

// ------------------------------------------------------------- pools ----

#[derive(Clone, Debug)]
struct CacheEntry {
    size: f64,
    last: f64,
    /// Release order (breaks ties in `last`: entries released at the same
    /// instant age in the order they were released).
    seq: u64,
    snap: Vec<f64>,
}

#[derive(Clone, Copy)]
struct WaitingHold {
    sid: usize,
    resumed: bool,
}

struct PoolState {
    cap: f64,
    block: Option<f64>,
    used: f64,
    cached: f64,
    holders: Vec<usize>,
    /// Cached prefixes by session serial (a session may have ended).
    entries: HashMap<u64, CacheEntry>,
    /// Waiting holds in enqueue order; policy keys are never cached.
    queue: VecDeque<WaitingHold>,
    growers: VecDeque<usize>,
    // statistics
    used_avg: TimeAverage,
    cached_avg: TimeAverage,
    queue_avg: TimeAverage,
    holders_avg: TimeAverage,
    wait: Welford,
    admissions: u64,
    evicted_entries: u64,
    evicted_units: f64,
    preemptions: u64,
    spills: u64,
    rejected: u64,
    /// Sessions preempted again without having advanced past the position
    /// of their previous preemption (a self-preemption livelock, typically).
    stuck: u64,
}

// ------------------------------------------------------------ stages ----

#[derive(Clone, Copy, Debug, PartialEq)]
struct OrdF64(f64);
impl Eq for OrdF64 {}
impl PartialOrd for OrdF64 {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for OrdF64 {
    fn cmp(&self, o: &Self) -> Ordering {
        self.0.total_cmp(&o.0)
    }
}

struct Job {
    owner: Option<usize>,
    /// Remaining work (fifo, delay, step) or the virtual finish tag (ps).
    work: f64,
    mode: RunMode,
    growing: Option<usize>,
    enqueued: f64,
    started: Option<f64>,
}

struct Iter {
    epoch: u64,
    assign: Vec<(u64, f64)>,
}

enum Kind {
    Fifo {
        servers: usize,
        queue: VecDeque<u64>,
        active: Vec<u64>,
    },
    Ps {
        v: f64,
        v_last: f64,
        rate: f64,
        set: BTreeSet<(OrdF64, u64)>,
        epoch: u64,
        dirty: bool,
    },
    Delay,
    Step {
        residents: Vec<u64>,
        iter: Option<Iter>,
        epoch: u64,
    },
    /// A `ps(cap)` stage some run holds together with another: its jobs are
    /// flows of `Interp::flows`, their rates set by the program's `share`.
    Shared {
        cap: f64,
    },
}

/// A job that holds several shared stages at once (or one, on a shared
/// stage): its remaining work, the rate the policy gives it, and its stages,
/// the first of which carries its `Status::InStage`.
struct Flow {
    stages: Vec<usize>,
    remaining: f64,
    rate: f64,
}

struct StageState {
    kind: Kind,
    jobs: HashMap<u64, Job>,
    est: PriceEstimator,
    number_avg: TimeAverage,
    busy_avg: TimeAverage,
    completed: u64,
    wait: Welford,
    service: Welford,
    iterations: u64,
    steps: StepStats,
}

/// What a step stage's iterations carried: how long each kind of batch
/// ran (prefill only, decode only, both), the decodes in the running
/// iteration, and per iteration that carried a decode its decodes and
/// its duration: the batch a decode is in and the step it waits for.
struct StepStats {
    prefill_only: TimeAverage,
    decode_only: TimeAverage,
    mixed: TimeAverage,
    decodes: TimeAverage,
    batch: Welford,
    duration: Welford,
    /// The gaps between a request's successive tokens that end here,
    /// after warm-up (`Interp::token`).
    itl: LogHistogram,
}

impl StepStats {
    fn new() -> Self {
        Self {
            prefill_only: TimeAverage::new(0.0, 0.0),
            decode_only: TimeAverage::new(0.0, 0.0),
            mixed: TimeAverage::new(0.0, 0.0),
            decodes: TimeAverage::new(0.0, 0.0),
            batch: Welford::new(),
            duration: Welford::new(),
            itl: LogHistogram::default(),
        }
    }

    /// An iteration of `ndec` decodes and `npre` prefill tokens starts at
    /// `now` (`ndec == npre == 0`: one that only preempted, or one ended).
    fn set(&mut self, now: f64, ndec: f64, npre: f64) {
        let (p, d) = (npre > 0.0, ndec > 0.0);
        self.prefill_only.set(now, f64::from(p && !d));
        self.decode_only.set(now, f64::from(d && !p));
        self.mixed.set(now, f64::from(p && d));
        self.decodes.set(now, ndec);
    }

    fn reset(&mut self, now: f64) {
        self.prefill_only.reset(now);
        self.decode_only.reset(now);
        self.mixed.reset(now);
        self.decodes.reset(now);
    }
}

// ------------------------------------------------------------ interp ----

#[derive(Clone, Copy)]
enum Which {
    Arrival,
    Workload,
    Session,
    Evict,
}

#[derive(Clone, Default)]
struct Ctx {
    sid: Option<usize>,
    snap: Option<Vec<f64>>,
    size: f64,
    age: f64,
    waited: f64,
    last: f64,
    queued: f64,
    n: f64,
    ntok: f64,
    ndec: f64,
    npre: f64,
    nres: f64,
    kvb: f64,
    kvp: f64,
    attn: f64,
    decoding: f64,
    admission: f64,
    remaining: f64,
}

impl Ctx {
    fn session(sid: usize) -> Self {
        Ctx {
            sid: Some(sid),
            ..Default::default()
        }
    }
}

struct ObserveStat {
    w: Welford,
    samples: Vec<f64>,
    /// (time, session serial, turn number) of every sample
    records: Vec<(f64, u64, u32)>,
}

pub struct Interp<'p> {
    p: &'p Linked,
    /// First error raised by the running program. The event loop stops before
    /// producing a report; internal invariant failures remain panics.
    error: Option<String>,
    now: f64,
    warm: bool,
    heap: BinaryHeap<Entry>,
    seq: u64,
    sessions: Vec<Session<'p>>,
    free: Vec<usize>,
    by_serial: HashMap<u64, usize>,
    next_serial: u64,
    next_job: u64,
    /// The flows on shared stages, their rates as of `flows_last`.
    flows: HashMap<u64, Flow>,
    flows_last: f64,
    /// Bumped at every recomputation: a finish scheduled before is stale.
    flow_epoch: u64,
    flows_dirty: bool,
    /// Stages whose flows changed since the last recomputation.
    flows_touched: Vec<usize>,
    pools: Vec<PoolState>,
    stages: Vec<StageState>,
    ready: VecDeque<usize>,
    rng_arr: StdRng,
    rng_session: StdRng,
    rng_evict: StdRng,
    rng_trace: StdRng,
    trace: Option<Corpus>,
    observes: Vec<ObserveStat>,
    /// Per gauge: its change points `(time, value)`, the value from that
    /// time on, one point per instant (the state after the instant's last
    /// event).
    gauges: Vec<Vec<(f64, f64)>>,
    /// Per pool: its eviction keys do not change while other entries are
    /// evicted (no pool or stage queries, no sampling), so `make_room` can
    /// key every entry once.
    evict_static: Vec<bool>,
    /// Debug switches (`SERQ_TRACE_EVICT`, `SERQ_TRACE_ITER`), read once.
    trace_evict: bool,
    trace_iter: bool,
    live: usize,
    live_avg: TimeAverage,
    /// While a stage admits from a queue it serves: (stage, budget left).
    admit_budget: Option<(usize, f64)>,
    next_dead: u64,
    next_lease: u64,
    /// Whether a preemption happened since the step stage's iteration
    /// began to be scheduled (vLLM's `preempted_reqs`, scheduler.py:869).
    preempted: bool,
    next_adm: u64,
    next_release: u64,
    arrivals: u64,
    ended: u64,
    turns: u64,
    events: u64,
}

/// Keys of dead cache entries (units past a `reuse` bound) start here, above
/// every session serial.
const DEAD_ENTRY: u64 = 1 << 62;

/// Which pool a `hold` waits at: the first one listed.
fn first_pool(pending: &Pending) -> usize {
    pending.pools[0].0
}

impl<'p> Interp<'p> {
    pub fn new(p: &'p Linked, trace: Option<Corpus>) -> Self {
        let seed = p.seed;
        let pools = p
            .pools
            .iter()
            .map(|cp| PoolState {
                cap: cp.cap,
                block: cp.block,
                used: 0.0,
                cached: 0.0,
                holders: vec![],
                entries: HashMap::new(),
                queue: VecDeque::new(),
                growers: VecDeque::new(),
                used_avg: TimeAverage::new(0.0, 0.0),
                cached_avg: TimeAverage::new(0.0, 0.0),
                queue_avg: TimeAverage::new(0.0, 0.0),
                holders_avg: TimeAverage::new(0.0, 0.0),
                wait: Welford::new(),
                admissions: 0,
                evicted_entries: 0,
                evicted_units: 0.0,
                preemptions: 0,
                spills: 0,
                rejected: 0,
                stuck: 0,
            })
            .collect();
        let shared = p.shared_stages();
        let stages = p
            .stages
            .iter()
            .enumerate()
            .map(|(k, cs)| StageState {
                kind: match &cs.kind {
                    CStageKind::Fifo(c) => Kind::Fifo {
                        servers: *c,
                        queue: VecDeque::new(),
                        active: vec![],
                    },
                    CStageKind::Ps(CExpr::Num(cap)) if shared[k] => Kind::Shared { cap: *cap },
                    CStageKind::Ps(_) => Kind::Ps {
                        v: 0.0,
                        v_last: 0.0,
                        rate: 0.0,
                        set: BTreeSet::new(),
                        epoch: 0,
                        dirty: false,
                    },
                    CStageKind::Delay => Kind::Delay,
                    CStageKind::Step(_) => Kind::Step {
                        residents: vec![],
                        iter: None,
                        epoch: 0,
                    },
                },
                jobs: HashMap::new(),
                est: PriceEstimator::default(),
                number_avg: TimeAverage::new(0.0, 0.0),
                busy_avg: TimeAverage::new(0.0, 0.0),
                completed: 0,
                wait: Welford::new(),
                service: Welford::new(),
                iterations: 0,
                steps: StepStats::new(),
            })
            .collect();
        Interp {
            p,
            error: None,
            now: 0.0,
            warm: p.warmup <= 0.0,
            heap: BinaryHeap::new(),
            seq: 0,
            sessions: vec![],
            free: vec![],
            by_serial: HashMap::new(),
            next_serial: 0,
            next_job: 0,
            flows: HashMap::new(),
            flows_last: 0.0,
            flow_epoch: 0,
            flows_dirty: false,
            flows_touched: vec![],
            pools,
            stages,
            ready: VecDeque::new(),
            rng_arr: StdRng::seed_from_u64(seed),
            rng_session: StdRng::seed_from_u64(seed ^ 0x5851_f42d_4c95_7f2d),
            rng_evict: StdRng::seed_from_u64(seed ^ 0x2545_f491_4f6c_dd1d),
            rng_trace: StdRng::seed_from_u64(seed ^ 0x6a09_e667_f3bc_c908),
            trace,
            evict_static: p
                .pools
                .iter()
                .map(|q| match &q.evict {
                    CEvict::Lru => true,
                    CEvict::By(keys) => keys.iter().all(static_key),
                })
                .collect(),
            trace_evict: std::env::var_os("SERQ_TRACE_EVICT").is_some(),
            trace_iter: std::env::var_os("SERQ_TRACE_ITER").is_some(),
            observes: p
                .observes
                .iter()
                .map(|_| ObserveStat {
                    w: Welford::new(),
                    samples: vec![],
                    records: vec![],
                })
                .collect(),
            gauges: p.gauges.iter().map(|_| vec![]).collect(),
            live: 0,
            live_avg: TimeAverage::new(0.0, 0.0),
            admit_budget: None,
            next_dead: 0,
            next_lease: 0,
            preempted: false,
            next_adm: 0,
            next_release: 0,
            arrivals: 0,
            ended: 0,
            turns: 0,
            events: 0,
        }
    }

    fn at(&mut self, time: f64, ev: Ev) {
        debug_assert!(
            time.is_finite() && time >= self.now,
            "event at {time} < {}",
            self.now
        );
        self.heap.push(Entry {
            time,
            seq: self.seq,
            ev,
        });
        self.seq += 1;
    }

    fn rng(&mut self, w: Which) -> &mut StdRng {
        match w {
            Which::Arrival => &mut self.rng_arr,
            Which::Workload => unreachable!("a workload draw is a session's (`eval`, Sample)"),
            Which::Session => &mut self.rng_session,
            Which::Evict => &mut self.rng_evict,
        }
    }

    // ------------------------------------------------------- running ----

    /// The next renewal gap, or a program error when it is not a positive
    /// time (a draw the linker could not see, #269).
    fn renewal_gap(&mut self, e: &CExpr) -> Option<f64> {
        let gap = self.eval(e, &Ctx::default(), Which::Arrival);
        if gap.is_finite() && gap > 0.0 {
            return Some(gap);
        }
        self.error = Some(format!(
            "`arrive renewal({})`: the interarrival time is {gap}, not a positive time",
            self.p.show_expr(e)
        ));
        None
    }

    /// Run to the horizon and return the report.
    pub fn run(mut self) -> Result<Report, String> {
        let p = self.p;
        if p.warmup > 0.0 {
            self.at(p.warmup, Ev::EndWarmup);
        }
        match &p.arrival {
            CArrival::Poisson(_) => self.at(0.0, Ev::Arrive),
            CArrival::Renewal(e) => {
                if let Some(gap) = self.renewal_gap(e) {
                    self.at(gap, Ev::Arrive);
                }
            }
            &CArrival::Closed(n) | &CArrival::Batch(n) => {
                for _ in 0..n {
                    self.spawn();
                }
            }
            CArrival::Sessions(ss) => {
                for (k, s) in ss.iter().enumerate() {
                    let script = (!s.turns.is_empty()).then_some(k);
                    self.spawn_with(&s.attrs, script);
                }
            }
            CArrival::None => {}
        }
        self.settle();
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        while let Some(e) = self.heap.peek() {
            let arrivals_done = p.arrivals.is_some_and(|n| self.arrivals >= n as u64);
            if arrivals_done && self.live == 0 {
                break;
            }
            if e.time > p.horizon {
                break;
            }
            // the clock moves on: the instant behind it is over, and what its
            // last event left is what the gauges read
            if e.time > self.now {
                self.read_gauges();
                if let Some(error) = self.error.take() {
                    return Err(error);
                }
            }
            let e = self.heap.pop().unwrap();
            self.now = e.time;
            self.events += 1;
            self.handle(e.ev);
            self.settle();
            if let Some(error) = self.error.take() {
                return Err(error);
            }
        }
        self.read_gauges();
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        if let Some(n) = p.arrivals {
            if self.arrivals < n as u64 {
                return Err(format!(
                    "run: horizon {} reached before requested arrivals: got {}, requested {n}",
                    p.horizon, self.arrivals
                ));
            }
            if self.live != 0 {
                return Err(format!(
                    "run: failed to drain {0} active sessions within horizon {1}",
                    self.live, p.horizon
                ));
            }
            if self.now <= p.warmup {
                return Err(format!(
                    "run: arrivals drained at {} before or at warmup {}; no measurement interval",
                    self.now, p.warmup
                ));
            }
        } else {
            self.now = p.horizon;
        }
        Ok(self.report())
    }

    /// Evaluate every gauge on the state an instant's last event left (the
    /// run calls it when the clock is about to move, and at the end),
    /// recording a change point when the value moved.
    fn read_gauges(&mut self) {
        let p = self.p;
        for (k, g) in p.gauges.iter().enumerate() {
            let v = self.eval(&g.expr, &Ctx::default(), Which::Session);
            let now = self.now;
            let pts = &mut self.gauges[k];
            match pts.last_mut() {
                Some(last) if last.0 == now => {
                    last.1 = v;
                    // back to what the previous instant held: no change
                    let n = pts.len();
                    if n >= 2 && pts[n - 2].1.total_cmp(&v).is_eq() {
                        pts.pop();
                    }
                }
                Some(last) if last.1.total_cmp(&v).is_eq() => {}
                _ => pts.push((now, v)),
            }
        }
    }

    fn handle(&mut self, ev: Ev) {
        match ev {
            Ev::EndWarmup => {
                self.warm = true;
                let now = self.now;
                self.live_avg.reset(now);
                for st in &mut self.stages {
                    st.number_avg.reset(now);
                    st.busy_avg.reset(now);
                    st.steps.reset(now);
                }
                for pl in &mut self.pools {
                    pl.used_avg.reset(now);
                    pl.cached_avg.reset(now);
                    pl.queue_avg.reset(now);
                    pl.holders_avg.reset(now);
                }
            }
            Ev::Arrive => {
                match self.p.arrival.clone() {
                    CArrival::Poisson(rate) if self.may_schedule_open_arrival() => {
                        let gap = -(1.0 - self.rng_arr.random::<f64>()).ln() / rate;
                        self.at(self.now + gap, Ev::Arrive);
                    }
                    CArrival::Renewal(e) if self.may_schedule_open_arrival() => {
                        if let Some(gap) = self.renewal_gap(&e) {
                            self.at(self.now + gap, Ev::Arrive);
                        }
                    }
                    _ => {}
                }
                self.spawn();
            }
            Ev::Finish { stage, job, epoch } => self.on_finish(stage, job, epoch),
            Ev::IterEnd { stage, epoch } => self.on_iter_end(stage, epoch),
            Ev::LeaseEnd { sid, serial, id } => {
                if self.sessions[sid].serial == serial
                    && let Some(i) = self.sessions[sid].leases.iter().position(|l| l.id == id)
                {
                    self.end_lease(sid, i);
                    self.try_admit_all();
                }
            }
        }
    }

    fn may_schedule_open_arrival(&self) -> bool {
        self.p
            .arrivals
            .is_none_or(|n| self.arrivals < (n as u64).saturating_sub(1))
    }

    fn settle(&mut self) {
        if self.error.is_some() {
            return;
        }
        loop {
            while let Some(sid) = self.ready.pop_front() {
                let now = self.now;
                let s = &mut self.sessions[sid];
                if s.readies.0 != now {
                    s.readies = (now, 0);
                }
                s.readies.1 += 1;
                if s.readies.1 > READIES_PER_INSTANT {
                    self.error = Some(format!(
                        "the instant t = {now} does not settle: session {} became ready {} times \
                         without time passing (a `run` of zero work, or a `hold` admitted and released, \
                         inside a loop)",
                        s.serial, s.readies.1
                    ));
                    return;
                }
                if self.sessions[sid].status == Status::Ready {
                    self.exec(sid);
                    if self.error.is_some() {
                        return;
                    }
                }
            }
            for pl in 0..self.pools.len() {
                self.retry_growers(pl);
                self.try_admit(pl);
                if self.error.is_some() {
                    return;
                }
            }
            if self.ready.is_empty() {
                break;
            }
        }
        // The memory invariant, `allocated + cached <= cap` in every
        // reachable configuration (`SerqLang.Step.invariant` proves it for
        // the pool relation; this checks the interpreter on every run of the
        // tests, the oracle and the differential tests).
        #[cfg(debug_assertions)]
        for (cp, pl) in self.p.pools.iter().zip(&self.pools) {
            debug_assert!(
                pl.used + pl.cached <= pl.cap + 1e-6,
                "pool `{}`: used {} + cached {} > cap {} at t = {}",
                cp.name,
                pl.used,
                pl.cached,
                pl.cap,
                self.now
            );
        }
        // An iteration starts only once every event of this instant has
        // been handled (a scheduler step sees all the arrivals up to it).
        let pending_now = self.heap.peek().is_some_and(|e| e.time <= self.now);
        for s in 0..self.stages.len() {
            match &self.stages[s].kind {
                Kind::Step {
                    iter: None,
                    residents,
                    ..
                } if (!residents.is_empty() || self.bound_waiting(s)) && !pending_now => {
                    self.start_iteration(s);
                    if self.error.is_some() {
                        return;
                    }
                }
                Kind::Ps { dirty: true, .. } => self.ps_reschedule(s),
                _ => {}
            }
        }
        if self.flows_dirty {
            self.flows_reschedule();
        }
        self.record();
    }

    fn record(&mut self) {
        let now = self.now;
        self.live_avg.set(now, self.live as f64);
        let flows = &self.flows;
        for st in &mut self.stages {
            let n = st.jobs.len() as f64;
            st.number_avg.set(now, n);
            let busy = match st.kind {
                // the capacity the flows carry, not whether any is there: a
                // `bottleneck` flow leaves the rest of its other stages unused
                Kind::Shared { cap } => {
                    // summed in job order: a HashMap's order is not the
                    // seed's, and a float sum depends on it
                    let mut ids: Vec<u64> = st.jobs.keys().copied().collect();
                    ids.sort_unstable();
                    ids.iter()
                        .map(|id| flows.get(id).map_or(0.0, |f| f.rate))
                        .sum::<f64>()
                        / cap
                }
                _ => {
                    if n > 0.0 {
                        1.0
                    } else {
                        0.0
                    }
                }
            };
            st.busy_avg.set(now, busy);
        }
        for pl in &mut self.pools {
            pl.used_avg.set(now, pl.used);
            pl.cached_avg.set(now, pl.cached);
            pl.queue_avg.set(now, pl.queue.len() as f64);
            pl.holders_avg.set(now, pl.holders.len() as f64);
        }
    }

    // ------------------------------------------------------ sessions ----

    fn spawn(&mut self) {
        self.spawn_with(&[], None);
    }

    fn spawn_with(&mut self, preset: &[(usize, f64)], script: Option<usize>) {
        let serial = self.next_serial;
        self.next_serial += 1;
        let mut attrs = vec![0.0; self.p.attrs.len()];
        attrs[self.p.slot_serial] = serial as f64;
        let trace = self.trace.as_ref().filter(|_| script.is_none()).map(|c| {
            let i = if self.p.trace_ordered {
                (serial as usize) % c.sessions.len()
            } else {
                self.rng_trace.random_range(0..c.sessions.len())
            };
            (i, 0usize)
        });
        let s = Session {
            serial,
            attrs,
            frames: vec![Frame {
                block: self.p.session,
                pc: 0,
                kind: FrameKind::Plain,
            }],
            status: Status::Ready,
            holds: vec![],
            pending: None,
            trace,
            script: script.map(|k| (k, 0)),
            adm_seq: u64::MAX,
            preempt_pos: HashMap::new(),
            stuck: false,
            leases: vec![],
            last_token: None,
            rng_wl: substream(self.p.seed, serial, 0, STREAM_WORKLOAD),
            rng: substream(self.p.seed, serial, 0, STREAM_SESSION),
            turn_count: 0,
            readies: (f64::NAN, 0),
        };
        let sid = match self.free.pop() {
            Some(i) => {
                self.sessions[i] = s;
                i
            }
            None => {
                self.sessions.push(s);
                self.sessions.len() - 1
            }
        };
        self.by_serial.insert(serial, sid);
        self.live += 1;
        self.arrivals += 1;
        // `init` runs at arrival, before the first `turn`; an explicit
        // session's preset attributes then override it.
        let init = self.p.init;
        self.exec_workload_block(sid, init);
        for &(slot, v) in preset {
            self.sessions[sid].attrs[slot] = v;
        }
        self.ready.push_back(sid);
    }

    fn exec_workload_block(&mut self, sid: usize, block: BlockId) {
        let p = self.p;
        let n = p.blocks[block].len();
        for i in 0..n {
            match &p.blocks[block][i] {
                CStmt::Set(slot, e) => {
                    let v = self.eval(e, &Ctx::session(sid), Which::Workload);
                    self.sessions[sid].attrs[*slot] = v;
                }
                CStmt::Observe(k, e) => {
                    let v = self.eval(e, &Ctx::session(sid), Which::Workload);
                    self.observe_for(sid, *k, v);
                }
                _ => unreachable!("workload blocks only set and observe"),
            }
        }
    }

    fn observe_for(&mut self, sid: usize, k: usize, v: f64) {
        if self.warm {
            let s = &self.sessions[sid];
            let rec = (self.now, s.serial, s.attrs[self.p.slot_turn] as u32);
            self.observes[k].w.push(v);
            self.observes[k].samples.push(v);
            self.observes[k].records.push(rec);
        }
    }

    fn do_turn(&mut self, sid: usize) {
        let p = self.p;
        self.sessions[sid].attrs[p.slot_turn] += 1.0;
        if self.warm {
            self.turns += 1;
        }
        // this turn's marks come from the stream of (seed, session, turn)
        self.sessions[sid].turn_count += 1;
        let turn_count = self.sessions[sid].turn_count;
        let serial = self.sessions[sid].serial;
        self.sessions[sid].rng_wl = substream(p.seed, serial, turn_count, STREAM_WORKLOAD);
        if let Some((k, ti)) = self.sessions[sid].script {
            let CArrival::Sessions(ss) = &p.arrival else {
                unreachable!("explicit turns come from explicit sessions")
            };
            let turns = &ss[k].turns;
            if ti < turns.len() {
                for &(slot, v) in &turns[ti] {
                    self.sessions[sid].attrs[slot] = v;
                }
                self.sessions[sid].attrs[p.slot_more] =
                    if ti + 1 < turns.len() { 1.0 } else { 0.0 };
                self.sessions[sid].script = Some((k, ti + 1));
            } else {
                self.sessions[sid].attrs[p.slot_more] = 0.0;
            }
        } else if let Some((ci, ti)) = self.sessions[sid].trace {
            let c = self.trace.as_ref().unwrap();
            let turns = &c.sessions[ci].turns;
            if ti < turns.len() {
                let t = turns[ti];
                let a = &mut self.sessions[sid].attrs;
                a[p.slot_new] = t.new;
                a[p.slot_out] = t.out;
                a[p.slot_think] = t.think;
                a[p.slot_more] = if ti + 1 < turns.len() { 1.0 } else { 0.0 };
                a[p.slot_forced] = t.forced;
                self.sessions[sid].trace = Some((ci, ti + 1));
            } else {
                self.sessions[sid].attrs[p.slot_more] = 0.0;
            }
        }
        let turn = p.turn;
        self.exec_workload_block(sid, turn);
    }

    fn end_session(&mut self, sid: usize) {
        if self.sessions[sid].status == Status::Ended {
            return;
        }
        self.detach(sid);
        while let Some(h) = self.sessions[sid].holds.pop() {
            self.release_hold(sid, &h);
        }
        while !self.sessions[sid].leases.is_empty() {
            self.end_lease(sid, 0);
        }
        self.sessions[sid].status = Status::Ended;
        self.sessions[sid].frames.clear();
        let serial = self.sessions[sid].serial;
        // A session's cached prefixes stay: the cache does not know that
        // the session has left (lecture [End] removes the session, not its
        // entries; vLLM keeps the blocks in the free queue). They age and
        // are evicted in order. A program that models dropping them writes
        // `drop POOL;` before `end;`.
        self.by_serial.remove(&serial);
        self.free.push(sid);
        self.live -= 1;
        if self.warm {
            self.ended += 1;
        }
        if let CArrival::Closed(_) = self.p.arrival {
            self.spawn();
        }
    }

    /// Take the session out of whatever it is waiting for or running at.
    fn detach(&mut self, sid: usize) {
        match self.sessions[sid].status.clone() {
            Status::Queued(pl) => {
                self.pools[pl].queue.retain(|entry| entry.sid != sid);
                self.sessions[sid].pending = None;
            }
            Status::InStage(st, job) => self.remove_job(st, job),
            Status::Growing(pl, _, job) => {
                self.pools[pl].growers.retain(|&s| s != sid);
                if let Some((st, j)) = job {
                    self.remove_job(st, j);
                }
            }
            Status::Ready | Status::Ended => {}
        }
        self.sessions[sid].status = Status::Ready;
    }

    /// Execute commands of a ready session until it blocks or ends.
    fn exec(&mut self, sid: usize) {
        let p = self.p;
        let mut steps: u64 = 0;
        loop {
            if self.error.is_some() {
                return;
            }
            if self.sessions[sid].status != Status::Ready {
                return;
            }
            steps += 1;
            if steps > STEPS_PER_INSTANT {
                self.error = Some(format!(
                    "session {} executes statements at t = {} without reaching a `run`, a `hold` \
                     that waits, or `end` ({STEPS_PER_INSTANT} statements: a loop that never blocks)",
                    self.sessions[sid].serial, self.now
                ));
                return;
            }
            let Some(fr) = self.sessions[sid].frames.last().cloned() else {
                self.end_session(sid);
                return;
            };
            let block = &p.blocks[fr.block];
            if fr.pc >= block.len() {
                match fr.kind {
                    FrameKind::Loop => {
                        self.sessions[sid].frames.last_mut().unwrap().pc = 0;
                    }
                    FrameKind::Plain => {
                        self.sessions[sid].frames.pop();
                    }
                    FrameKind::Hold => {
                        self.sessions[sid].frames.pop();
                        let h = self.sessions[sid]
                            .holds
                            .pop()
                            .expect("hold frame has a hold");
                        self.end_hold(sid, &h);
                        // this hold completed: its pools' preemption positions
                        // are history (an enclosing hold keeps its own), and
                        // there is nothing to resume from
                        for &(q, _) in &h.pools {
                            self.sessions[sid].preempt_pos.remove(&q);
                        }
                        let slot_computed = self.p.slot_computed;
                        self.sessions[sid].attrs[slot_computed] = 0.0;
                        self.try_admit_all();
                    }
                }
                continue;
            }
            self.sessions[sid].frames.last_mut().unwrap().pc += 1;
            let stmt = &block[fr.pc];
            match stmt {
                CStmt::Turn => self.do_turn(sid),
                CStmt::Set(slot, e) => {
                    let v = self.eval(e, &Ctx::session(sid), Which::Session);
                    self.sessions[sid].attrs[*slot] = v;
                }
                CStmt::Observe(k, e) => {
                    let v = self.eval(e, &Ctx::session(sid), Which::Session);
                    self.observe_for(sid, *k, v);
                }
                CStmt::End => {
                    self.end_session(sid);
                    return;
                }
                CStmt::Loop(b) => self.sessions[sid].frames.push(Frame {
                    block: *b,
                    pc: 0,
                    kind: FrameKind::Loop,
                }),
                CStmt::Branch(pe, a, b) => {
                    // a guard is a test: 0 or 1. A draw is written `branch
                    // with (p)`, which the parser rewrites to a Bernoulli
                    // sample, so the guard never sees a fraction on purpose
                    let pr = self.eval(pe, &Ctx::session(sid), Which::Session);
                    let take = if pr == 1.0 {
                        true
                    } else if pr == 0.0 {
                        false
                    } else {
                        self.error = Some(format!(
                            "`branch ({})`: the guard is {pr}, not 0 or 1; a draw is written `branch with (p)`",
                            self.p.show_expr(pe)
                        ));
                        return;
                    };
                    let blk = if take { *a } else { *b };
                    self.sessions[sid].frames.push(Frame {
                        block: blk,
                        pc: 0,
                        kind: FrameKind::Plain,
                    });
                }
                CStmt::Choose { var, count, key } => {
                    // a count of members, a whole number (#270)
                    let x = self.eval(count, &Ctx::session(sid), Which::Session);
                    if !(x >= 0.0 && x.fract() == 0.0) {
                        self.error = Some(format!(
                            "`choose … in ({})`: the count is {x}, not a whole number",
                            self.p.show_expr(count)
                        ));
                        return;
                    }
                    let n = x as usize;
                    // ascending keys, lexicographic, ties to the smallest index
                    let mut best: Option<(Vec<f64>, usize)> = None;
                    for j in 0..n {
                        self.sessions[sid].attrs[*var] = j as f64;
                        let k: Vec<f64> = key
                            .iter()
                            .map(|e| self.eval(e, &Ctx::session(sid), Which::Session))
                            .collect();
                        if best
                            .as_ref()
                            .is_none_or(|b| k.partial_cmp(&b.0) == Some(std::cmp::Ordering::Less))
                        {
                            best = Some((k, j));
                        }
                    }
                    self.sessions[sid].attrs[*var] = best.map_or(0.0, |b| b.1 as f64);
                }
                CStmt::Hold {
                    pools,
                    reuse,
                    body,
                    cache,
                    lease,
                } => {
                    let mut ps = vec![];
                    let mut exprs = vec![];
                    let mut reserve = vec![];
                    for (r, e, f) in pools {
                        let pl = self.session_index(r, sid);
                        let units = self.amount(e, sid, &format!("hold {}", self.p.pools[pl].name));
                        ps.push((pl, units));
                        exprs.push(e);
                        reserve.push(f.as_ref());
                    }
                    let lease = lease.as_ref().map(|(r, t)| (self.session_index(r, sid), t));
                    let n = ps.len();
                    let pending = Pending {
                        pools: ps,
                        exprs,
                        reserve,
                        need: vec![0.0; n],
                        reuse: reuse.as_ref(),
                        cache: cache.as_ref(),
                        lease,
                        body: *body,
                        queued_at: self.now,
                    };
                    self.enqueue_hold(sid, pending, false);
                    return;
                }
                CStmt::Grow(r, e) => {
                    let pl = self.session_index(r, sid);
                    let units = self.amount(e, sid, &format!("grow {}", self.p.pools[pl].name));
                    if !self.grow(sid, pl, units) {
                        return;
                    }
                }
                CStmt::Drop(r) => {
                    let pl = self.session_index(r, sid);
                    let serial = self.sessions[sid].serial;
                    self.remove_entry(pl, serial);
                }
                CStmt::Release(r) => {
                    let pl = self.session_index(r, sid);
                    self.release_early(sid, pl);
                    self.try_admit_all();
                }
                CStmt::Load(r, e) => {
                    let pl = self.session_index(r, sid);
                    let n = self.amount(e, sid, &format!("load {}", self.p.pools[pl].name));
                    self.load(sid, pl, n);
                }
                CStmt::Run {
                    stage,
                    mode,
                    work,
                    growing,
                    also,
                } => {
                    let st = self.session_index(stage, sid);
                    // the kernel's spelling: `decode on E (…)` is `run E decode (…)`
                    let m = match mode {
                        RunMode::Plain => "",
                        RunMode::Prefill => " prefill",
                        RunMode::Decode => " decode",
                    };
                    let what = format!("run {}{m}", self.p.stages[st].name);
                    let w = self.amount(work, sid, &what);
                    if matches!(self.stages[st].kind, Kind::Shared { .. }) {
                        let mut stages = vec![st];
                        for r in also {
                            stages.push(self.session_index(r, sid));
                        }
                        self.start_flow(stages, Some(sid), w);
                        return;
                    }
                    let g = growing.as_ref().map(|r| self.session_index(r, sid));
                    self.start_job(st, Some(sid), w, *mode, g);
                    return;
                }
            }
        }
    }

    /// The member a session statement's pool or stage reference names.
    fn session_index(&mut self, r: &CRef, sid: usize) -> usize {
        self.ref_index(r, &Ctx::session(sid), Which::Session)
    }

    /// The amount a session statement names (`what`: its keyword and target), which
    /// is a number of units, tokens or seconds: not NaN and not negative
    /// beyond rounding (#270). On a program error, 0 and the error set.
    fn amount(&mut self, e: &CExpr, sid: usize, what: &str) -> f64 {
        let x = self.eval(e, &Ctx::session(sid), Which::Session);
        if x.is_nan() || x < -EPS {
            if self.error.is_none() {
                self.error = Some(format!(
                    "`{what} ({})`: the amount is {x}, not a number of units, tokens or seconds",
                    self.p.show_expr(e)
                ));
            }
            return 0.0;
        }
        x.max(0.0)
    }

    // --------------------------------------------------------- pools ----

    /// Whether `units` more fit pool `pl` beside what is allocated (the
    /// cache not counted: it is evicted to make room).
    fn fits(&self, pl: usize, units: f64) -> bool {
        self.pools[pl].used + units <= self.pools[pl].cap + EPS
    }

    fn round_up(&self, pl: usize, units: f64) -> f64 {
        match self.pools[pl].block {
            Some(b) => (units / b).ceil() * b,
            None => units,
        }
    }

    fn round_down(&self, pl: usize, units: f64) -> f64 {
        match self.pools[pl].block {
            Some(b) => (units / b).floor() * b,
            None => units,
        }
    }

    /// Put a hold request in its pool's queue. `front`: a preempted
    /// session re-enters at the head (vLLM `waiting.prepend_request`).
    /// Returns false if the request can never fit (the session ends).
    fn enqueue_hold(&mut self, sid: usize, pending: Pending<'p>, front: bool) -> bool {
        for k in 0..pending.pools.len() {
            let (pl, units) = pending.pools[k];
            // what admission waits for: the units, or the reservation above them
            let need = match pending.reserve[k] {
                Some(f) => {
                    let what = format!("hold {} reserve", self.p.pools[pl].name);
                    self.amount(f, sid, &what).max(units)
                }
                None => units,
            };
            if self.round_up(pl, need) > self.pools[pl].cap {
                self.pools[pl].rejected += 1;
                self.end_session(sid);
                return false;
            }
        }
        let pl = first_pool(&pending);
        self.sessions[sid].pending = Some(pending);
        self.sessions[sid].status = Status::Queued(pl);
        let entry = WaitingHold {
            sid,
            resumed: front,
        };
        if front {
            self.pools[pl].queue.push_front(entry);
        } else {
            self.pools[pl].queue.push_back(entry);
        }
        true
    }

    fn try_admit_all(&mut self) {
        for pl in 0..self.pools.len() {
            self.retry_growers(pl);
            self.try_admit(pl);
        }
    }

    /// Select again after every admission: elapsed wait and scheduler state
    /// can change even while no new request enters the queue. Resumed holds
    /// keep vLLM's prepend priority; policy ties keep original queue order.
    fn next_waiter(&mut self, pl: usize) -> Option<(usize, usize)> {
        let queue = &self.pools[pl].queue;
        if let Some(entry) = queue.front()
            && (entry.resumed || self.p.pools[pl].queue.is_none())
        {
            return Some((0, entry.sid));
        }
        let keys = self.p.pools[pl].queue.as_ref()?;
        let candidates: Vec<_> = queue.iter().map(|entry| entry.sid).collect();
        let mut best: Option<(KeyOrd, usize, usize)> = None;
        for (index, sid) in candidates.into_iter().enumerate() {
            let mut ctx = Ctx::session(sid);
            ctx.waited = self.now - self.sessions[sid].pending.as_ref().unwrap().queued_at;
            let key = KeyOrd(
                keys.iter()
                    .map(|k| self.eval(k, &ctx, Which::Session))
                    .collect(),
            );
            if best.as_ref().is_none_or(|(old, _, _)| key < *old) {
                best = Some((key, index, sid));
            }
        }
        best.map(|(_, index, sid)| (index, sid))
    }

    /// The hold request of a queued session with its units evaluated now.
    fn pending_now(&mut self, sid: usize) -> Pending<'p> {
        let mut pending = self.sessions[sid]
            .pending
            .clone()
            .expect("queued session has a hold");
        for k in 0..pending.pools.len() {
            let e = pending.exprs[k];
            let pl = pending.pools[k].0;
            let u = self.amount(e, sid, &format!("hold {}", self.p.pools[pl].name));
            pending.pools[k].1 = u;
            pending.need[k] = match pending.reserve[k] {
                Some(f) => {
                    let what = format!("hold {} reserve", self.p.pools[pl].name);
                    self.amount(f, sid, &what).max(u)
                }
                None => u,
            };
        }
        pending
    }

    /// Admit the selected waiting hold while every pool of its hold
    /// has room; a selection that does not fit blocks the rest. A pool whose
    /// queue is served by a stage (`admit via`) is admitted from there.
    fn try_admit(&mut self, pl: usize) {
        if self.p.pools[pl].admit_via.is_some() {
            return;
        }
        while let Some((index, sid)) = self.next_waiter(pl) {
            let pending = self.pending_now(sid);
            // The guard counts only allocated units: cached prefixes never
            // block an admission (they are evicted as needed).
            let reserve = pending
                .pools
                .iter()
                .zip(&pending.need)
                .all(|(&(q, _), &need)| self.fits(q, self.round_up(q, need)));
            if !reserve {
                break;
            }
            self.pools[pl].queue.remove(index);
            self.admit(sid, pending);
        }
    }

    fn own_entry_size(&self, pl: usize, sid: usize) -> f64 {
        self.pools[pl]
            .entries
            .get(&self.sessions[sid].serial)
            .map_or(0.0, |e| e.size)
    }

    fn admit(&mut self, sid: usize, pending: Pending<'p>) {
        let p = self.p;
        let serial = self.sessions[sid].serial;
        let mut cached_first = None;
        let mut held = vec![];
        let mut pos = vec![];
        let reuse = pending
            .reuse
            .as_ref()
            .map(|e| self.eval(e, &Ctx::session(sid), Which::Session).max(0.0));
        for &(q, units) in &pending.pools {
            let need = self.round_up(q, units);
            // A hold with a `cache` clause takes part in the prefix cache:
            // it consumes the own prefix, at most `reuse` of it; the rest
            // stays cached as a dead entry of the same age (vLLM: the hit is
            // the longest run of cached blocks and the blocks past it are not
            // touched, so they keep their place in the free queue,
            // single_type_kv_cache_manager.py:743-838).
            // One without the clause is memory alone and leaves the
            // session's cached blocks where they are (#230: an outer hold
            // around the request's used to consume them at its admission,
            // so the request found none, and had no clause to put them back).
            let removed = if pending.cache.is_some() {
                self.remove_entry(q, serial)
            } else {
                None
            };
            let mut own = removed.as_ref().map_or(0.0, |e| e.size);
            if let Some(r) = reuse
                && let Some(removed) = &removed
            {
                let r = self.round_down(q, r);
                if own > r {
                    let dead = own - r;
                    own = r;
                    let key = DEAD_ENTRY + self.next_dead;
                    self.next_dead += 1;
                    let entry = CacheEntry {
                        seq: removed.seq,
                        size: dead,
                        last: removed.last,
                        snap: self.sessions[sid].attrs.clone(),
                    };
                    self.pools[q].entries.insert(key, entry);
                    self.pools[q].cached += dead;
                }
            }
            // `cached`: the consumed prefix (the largest, if several pools
            // of the hold had one)
            cached_first = Some(cached_first.map_or(own, |c: f64| c.max(own)));
            self.make_room(q, need);
            self.pools[q].used += need;
            self.pools[q].holders.push(sid);
            self.pools[q].admissions += 1;
            held.push((q, need));
            pos.push(own);
        }
        let pl = first_pool(&pending);
        if self.warm {
            let w = self.now - pending.queued_at;
            self.pools[pl].wait.push(w);
        }
        let seq = self.next_adm;
        self.next_adm += 1;
        let s = &mut self.sessions[sid];
        s.adm_seq = seq;
        s.attrs[p.slot_cached] = cached_first.unwrap_or(0.0);
        s.pending = None;
        s.status = Status::Ready;
        s.holds.push(Hold {
            pools: held,
            pos,
            grown: false,
            cache: pending.cache,
            lease: pending.lease,
            body: pending.body,
        });
        s.frames.push(Frame {
            block: pending.body,
            pc: 0,
            kind: FrameKind::Hold,
        });
        self.ready.push_back(sid);
    }

    /// Evict cached prefixes until `need` more units fit.
    fn make_room(&mut self, pl: usize, need: f64) {
        let over = |s: &Self| s.pools[pl].used + s.pools[pl].cached + need > s.pools[pl].cap + EPS;
        // one victim is a linear scan; the heap pays off only for several
        let short = self.pools[pl].used + self.pools[pl].cached + need - self.pools[pl].cap;
        let several = self.pools[pl].block.is_some_and(|b| short > b + EPS);
        if self.evict_static[pl] && several {
            // The keys of the entries not evicted do not change within one
            // call: key every entry once, then re-key only the one that
            // shrank. The same victims, in the same order, as `evict_one`.
            let serials: Vec<u64> = self.pools[pl].entries.keys().copied().collect();
            let mut heap: BinaryHeap<std::cmp::Reverse<(KeyOrd, u64)>> = serials
                .into_iter()
                .map(|s| std::cmp::Reverse((KeyOrd(self.entry_key(pl, s)), s)))
                .collect();
            while over(self) {
                let Some(std::cmp::Reverse((_, serial))) = heap.pop() else {
                    break;
                };
                if self.evict_entry(pl, serial) {
                    heap.push(std::cmp::Reverse((
                        KeyOrd(self.entry_key(pl, serial)),
                        serial,
                    )));
                }
            }
        } else {
            while over(self) {
                if !self.evict_one(pl) {
                    break;
                }
            }
        }
        debug_assert!(
            self.fits(pl, need),
            "make_room called without a passing guard"
        );
    }

    fn entry_key(&mut self, pl: usize, serial: u64) -> Vec<f64> {
        if let CEvict::Lru = &self.p.pools[pl].evict {
            let e = &self.pools[pl].entries[&serial];
            return vec![e.last, e.seq as f64];
        }
        let e = self.pools[pl].entries[&serial].clone();
        let live = self.by_serial.get(&serial).copied();
        let queued = live.is_some_and(|s| matches!(self.sessions[s].status, Status::Queued(_)));
        let ctx = Ctx {
            sid: live,
            snap: if live.is_some() {
                None
            } else {
                Some(e.snap.clone())
            },
            size: e.size,
            age: self.now - e.last,
            last: e.last,
            queued: if queued { 1.0 } else { 0.0 },
            ..Default::default()
        };
        let p = self.p;
        match &p.pools[pl].evict {
            CEvict::Lru => vec![e.last, e.seq as f64],
            CEvict::By(keys) => {
                let mut v: Vec<f64> = keys
                    .iter()
                    .map(|k| self.eval(k, &ctx, Which::Evict))
                    .collect();
                v.push(e.seq as f64);
                v
            }
        }
    }

    /// Evict the entry (or its tail block) with the smallest key.
    fn evict_one(&mut self, pl: usize) -> bool {
        let serials: Vec<u64> = self.pools[pl].entries.keys().copied().collect();
        if serials.is_empty() {
            return false;
        }
        let mut best: Option<(Vec<f64>, u64)> = None;
        for s in serials {
            let k = self.entry_key(pl, s);
            let better = match &best {
                None => true,
                Some((bk, _)) => lex_less(&k, bk),
            };
            if better {
                best = Some((k, s));
            }
        }
        let (_, serial) = best.unwrap();
        self.evict_entry(pl, serial);
        true
    }

    /// Evict entry `serial` (or its tail block); whether it is still cached.
    fn evict_entry(&mut self, pl: usize, serial: u64) -> bool {
        let block = self.pools[pl].block;
        let e = self.pools[pl].entries.get_mut(&serial).unwrap();
        let removed = match block {
            Some(b) => b.min(e.size),
            None => e.size,
        };
        e.size -= removed;
        if self.trace_evict {
            eprintln!("EVICT {:.4} {serial} {removed}", self.now);
        }
        let snap = e.snap.clone();
        let last = e.last;
        let gone = e.size <= EPS;
        if gone {
            self.pools[pl].entries.remove(&serial);
            self.pools[pl].evicted_entries += 1;
        }
        self.pools[pl].cached -= removed;
        self.pools[pl].evicted_units += removed;
        self.spill(pl, serial, removed, last, snap);
        !gone
    }

    fn spill(&mut self, pl: usize, serial: u64, units: f64, last: f64, snap: Vec<f64>) {
        let Some(sp) = self.p.pools[pl].spill.clone() else {
            return;
        };
        let live = self.by_serial.get(&serial).copied();
        // the same context an eviction key sees (`queued` included: the
        // spec lists it for spill predicates, and this used to leave it 0)
        let queued = live.is_some_and(|s| matches!(self.sessions[s].status, Status::Queued(_)));
        let ctx = Ctx {
            sid: live,
            snap: if live.is_some() {
                None
            } else {
                Some(snap.clone())
            },
            size: units,
            age: self.now - last,
            last,
            queued: if queued { 1.0 } else { 0.0 },
            ..Default::default()
        };
        if self.eval(&sp.when, &ctx, Which::Evict) == 0.0 {
            return;
        }
        let work = self.eval(&sp.work, &ctx, Which::Evict).max(0.0);
        let to = sp.to;
        self.make_room(to, 0.0);
        let now = self.now;
        // extend or create the tier entry
        let rseq = self.next_release;
        self.next_release += 1;
        let e = self.pools[to].entries.entry(serial).or_insert(CacheEntry {
            seq: rseq,
            size: 0.0,
            last: now,
            snap,
        });
        e.size += units;
        e.last = now;
        e.seq = rseq;
        self.pools[to].cached += units;
        self.pools[pl].spills += 1;
        self.start_job(sp.via, None, work, RunMode::Plain, None);
    }

    /// Remove a session's entry from a pool, and return it.
    fn remove_entry(&mut self, pl: usize, serial: u64) -> Option<CacheEntry> {
        let e = self.pools[pl].entries.remove(&serial)?;
        self.pools[pl].cached -= e.size;
        Some(e)
    }

    /// A preempted or ended hold gives everything back at once.
    fn release_hold(&mut self, sid: usize, h: &Hold<'p>) {
        for (k, &(q, alloc)) in h.pools.iter().enumerate() {
            // only what was computed can be cached: the position of a
            // growing hold, else the whole allocation
            let computed = if h.grown { h.pos[k] } else { alloc };
            self.release_units(sid, q, alloc, computed, h.cache);
        }
    }

    /// The scope's end: every pool is given back, except the leased one,
    /// whose allocation stays the session's (neither evictable nor a
    /// preemption victim) until its `release`, the expiry or the session's
    /// end.
    fn end_hold(&mut self, sid: usize, h: &Hold<'p>) {
        for (k, &(q, alloc)) in h.pools.iter().enumerate() {
            let computed = if h.grown { h.pos[k] } else { alloc };
            match h.lease {
                Some((lp, t)) if lp == q => {
                    let t = self.eval(t, &Ctx::session(sid), Which::Session).max(0.0);
                    let id = self.next_lease;
                    self.next_lease += 1;
                    self.sessions[sid].leases.push(Lease {
                        id,
                        pool: q,
                        alloc,
                        computed,
                        cache: h.cache,
                    });
                    if t.is_finite() {
                        let serial = self.sessions[sid].serial;
                        self.at(self.now + t, Ev::LeaseEnd { sid, serial, id });
                    }
                }
                _ => self.release_units(sid, q, alloc, computed, h.cache),
            }
        }
    }

    /// A lease ends: the units go back, `cache` applies.
    fn end_lease(&mut self, sid: usize, i: usize) {
        let l = self.sessions[sid].leases.remove(i);
        self.release_units(sid, l.pool, l.alloc, l.computed, l.cache);
    }

    /// Give `alloc` units of `q` back, keeping `min(cache, computed)` of them
    /// cached (rounded down to blocks) for the session.
    fn release_units(
        &mut self,
        sid: usize,
        q: usize,
        alloc: f64,
        computed: f64,
        cache: Option<&'p CExpr>,
    ) {
        let serial = self.sessions[sid].serial;
        self.pools[q].used -= alloc;
        self.pools[q].holders.retain(|&s| s != sid);
        if let Some(c) = cache {
            let want = self.eval(c, &Ctx::session(sid), Which::Session).max(0.0);
            let keep = self.round_down(q, want.min(computed));
            if keep > 0.0 {
                let snap = self.sessions[sid].attrs.clone();
                self.remove_entry(q, serial);
                let rseq = self.next_release;
                self.next_release += 1;
                self.pools[q].entries.insert(
                    serial,
                    CacheEntry {
                        seq: rseq,
                        size: keep,
                        last: self.now,
                        snap,
                    },
                );
                self.pools[q].cached += keep;
            }
        }
    }

    /// `release P`: the innermost hold on `pl` gives its allocation there
    /// back now, caching per its clause, and no longer holds `pl`; its scope
    /// end then has nothing left there. With no hold on `pl`, the session's
    /// lease of it ends (the transfer took the KV). Neither: a no-op (a
    /// hold re-executed after a preemption reaches the statement again).
    fn release_early(&mut self, sid: usize, pl: usize) {
        let Some(hi) = self.sessions[sid]
            .holds
            .iter()
            .rposition(|h| h.pools.iter().any(|&(q, _)| q == pl))
        else {
            if let Some(i) = self.sessions[sid].leases.iter().position(|l| l.pool == pl) {
                self.end_lease(sid, i);
            }
            return;
        };
        let h = &mut self.sessions[sid].holds[hi];
        let k = h.pools.iter().position(|&(q, _)| q == pl).unwrap();
        let (q, alloc) = h.pools.remove(k);
        let pos = h.pos.remove(k);
        let computed = if h.grown { pos } else { alloc };
        let cache = h.cache;
        self.release_units(sid, q, alloc, computed, cache);
        self.sessions[sid].preempt_pos.remove(&q);
    }

    /// `load P (n)`: the KV of `n` tokens arrived from outside the engine;
    /// the innermost hold's position on `pl` advances by `n`, which its
    /// allocation must cover (`grow` first, or allocate at admission, as
    /// vLLM's decoder allocates the whole prompt before it reads).
    fn load(&mut self, sid: usize, pl: usize, n: f64) {
        let name = &self.p.pools[pl].name;
        let Some(hi) = self.sessions[sid]
            .holds
            .iter()
            .rposition(|h| h.pools.iter().any(|&(q, _)| q == pl))
        else {
            self.error = Some(format!("`load {name}` outside a hold of `{name}`"));
            return;
        };
        let h = &mut self.sessions[sid].holds[hi];
        let k = h.pools.iter().position(|&(q, _)| q == pl).unwrap();
        let alloc = h.pools[k].1;
        if h.pos[k] + n > alloc + EPS {
            self.error = Some(format!(
                "`load {name} ({n})`: the hold has {alloc} allocated and {} computed; \
                 a load must fit the allocation (grow first)",
                h.pos[k]
            ));
            return;
        }
        h.pos[k] += n;
        h.grown = true;
    }

    /// Allocate `units` more for the innermost hold of `sid` on `pl`.
    /// Returns false if the session blocked (or was preempted).
    fn grow(&mut self, sid: usize, pl: usize, units: f64) -> bool {
        let Some(hi) = self.sessions[sid]
            .holds
            .iter()
            .rposition(|h| h.pools.iter().any(|&(q, _)| q == pl))
        else {
            self.error = Some(format!(
                "grow outside a hold of pool `{}`",
                self.p.pools[pl].name
            ));
            return false;
        };
        let alloc_now = self.sessions[sid].holds[hi]
            .pools
            .iter()
            .find(|&&(q, _)| q == pl)
            .unwrap()
            .1;
        let target = self.round_up(pl, alloc_now + units);
        let need = target - alloc_now;
        if need <= 0.0 {
            return true;
        }
        loop {
            if self.fits(pl, need) {
                self.make_room(pl, need);
                self.pools[pl].used += need;
                let h = &mut self.sessions[sid].holds[hi];
                for e in h.pools.iter_mut() {
                    if e.0 == pl {
                        e.1 += need;
                    }
                }
                return true;
            }
            let victim = match self.p.pools[pl].preempt {
                Preempt::None => None,
                Preempt::Lifo => self.lifo_victim(pl),
            };
            match victim {
                Some(victim) => {
                    self.preempt(victim, pl);
                    if victim == sid {
                        return false;
                    }
                }
                // `preempt none`, or nobody to preempt: wait for room
                None => {
                    let resume = match self.sessions[sid].status {
                        Status::InStage(st, j) => Some((st, j)),
                        _ => None,
                    };
                    self.sessions[sid].status = Status::Growing(pl, units, resume);
                    self.pools[pl].growers.push_back(sid);
                    return false;
                }
            }
        }
    }

    /// vLLM's `running[-1]` (scheduler.py:742-813): among the holders of
    /// `pl` that are residents of a step stage whose memory `pl` is, the one
    /// admitted last - by the session's latest admission, which is the
    /// residents' serving order (`running` is in order of scheduling, and a
    /// request that queued once more for a slot after its KV arrived took
    /// its place then, not when its blocks were allocated). A holder that
    /// has left the engine is not preempted: a prefiller's finished request
    /// keeps its blocks leased for the decoder's read and is in no `running`
    /// list, and a decoder's request waiting for that read
    /// (`WAITING_FOR_REMOTE_KVS`) holds its blocks and is not in `running`
    /// either; with no resident holding the pool there is nobody to preempt
    /// and the grower waits. A pool that is no engine's memory: its most
    /// recently admitted holder.
    fn lifo_victim(&self, pl: usize) -> Option<usize> {
        let engines: Vec<usize> = self
            .p
            .stages
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(&s.kind, CStageKind::Step(st) if st.memory == Some(pl)))
            .map(|(i, _)| i)
            .collect();
        let holders = &self.pools[pl].holders;
        if engines.is_empty() {
            // the last holder that holds the pool in a scope: a lease is
            // not preempted
            return holders.iter().copied().rev().find(|&s| {
                self.sessions[s]
                    .holds
                    .iter()
                    .any(|h| h.pools.iter().any(|&(q, _)| q == pl))
            });
        }
        holders
            .iter()
            .copied()
            .filter(|&s| {
                matches!(self.sessions[s].status, Status::InStage(x, _) if engines.contains(&x))
            })
            .max_by_key(|&s| self.sessions[s].adm_seq)
    }

    fn retry_growers(&mut self, pl: usize) {
        while let Some(&sid) = self.pools[pl].growers.front() {
            let Status::Growing(_, units, resume) = self.sessions[sid].status else {
                self.pools[pl].growers.pop_front();
                continue;
            };
            let hi = self.sessions[sid]
                .holds
                .iter()
                .rposition(|h| h.pools.iter().any(|&(q, _)| q == pl))
                .unwrap();
            let alloc_now = self.sessions[sid].holds[hi]
                .pools
                .iter()
                .find(|&&(q, _)| q == pl)
                .unwrap()
                .1;
            let need = self.round_up(pl, alloc_now + units) - alloc_now;
            if !self.fits(pl, need) {
                break;
            }
            self.pools[pl].growers.pop_front();
            self.make_room(pl, need);
            self.pools[pl].used += need;
            for e in self.sessions[sid].holds[hi].pools.iter_mut() {
                if e.0 == pl {
                    e.1 += need;
                }
            }
            match resume {
                Some((st, j)) => {
                    self.sessions[sid].status = Status::InStage(st, j);
                    // a growing job: the retried growth covers its pending tokens
                    self.advance_pos(sid, pl, units);
                }
                None => {
                    self.sessions[sid].status = Status::Ready;
                    self.ready.push_back(sid);
                }
            }
        }
    }

    /// `(allocated, position)` of the innermost hold of `sid` on `pl`.
    fn hold_alloc_pos(&mut self, sid: usize, pl: usize) -> (f64, f64) {
        let Some(h) = self.sessions[sid]
            .holds
            .iter()
            .rev()
            .find(|h| h.pools.iter().any(|&(q, _)| q == pl))
        else {
            self.error = Some(format!(
                "`growing {}` outside a hold of it",
                self.p.pools[pl].name
            ));
            return (0.0, 0.0);
        };
        let k = h.pools.iter().position(|&(q, _)| q == pl).unwrap();
        (h.pools[k].1, h.pos[k])
    }

    fn advance_pos(&mut self, sid: usize, pl: usize, tokens: f64) {
        let h = self.sessions[sid]
            .holds
            .iter_mut()
            .rev()
            .find(|h| h.pools.iter().any(|&(q, _)| q == pl))
            .unwrap();
        let k = h.pools.iter().position(|&(q, _)| q == pl).unwrap();
        h.pos[k] += tokens;
        h.grown = true;
    }

    /// Units of `pl` held by `sid` (all its holds).
    fn held_in(&self, sid: usize, pl: usize) -> f64 {
        self.sessions[sid]
            .holds
            .iter()
            .flat_map(|h| h.pools.iter())
            .filter(|&&(q, _)| q == pl)
            .map(|&(_, u)| u)
            .sum()
    }

    /// Preempt `victim`'s hold on `pl`: its job leaves its stage, the
    /// hold's allocations are released (cached), and it re-queues at the
    /// head of the pool's queue with the hold to execute again.
    fn preempt(&mut self, victim: usize, pl: usize) {
        let hi = self.sessions[victim]
            .holds
            .iter()
            .rposition(|h| h.pools.iter().any(|&(q, _)| q == pl))
            .expect("victim holds the pool");
        // what the hold has computed on this pool: its position there, which
        // starts at the cached prefix it consumed and advances with its
        // `growing` runs. Not the allocation: a holder preempted before its
        // first iteration has computed nothing of what it was allocated
        // (vLLM: `num_computed_tokens` is 0 for it, and it has no output)
        let computed = {
            let h = &self.sessions[victim].holds[hi];
            let k = h.pools.iter().position(|&(q, _)| q == pl).unwrap();
            h.pos[k]
        };
        if let Some(&prev) = self.sessions[victim].preempt_pos.get(&pl) {
            if computed <= prev + EPS && !self.sessions[victim].stuck {
                self.sessions[victim].stuck = true;
                self.pools[pl].stuck += 1;
            }
        }
        self.sessions[victim].preempt_pos.insert(pl, computed);
        if let Some(t) = &mut self.sessions[victim].last_token {
            t.preempted = true;
        }
        // the same value is what the re-executed hold resumes from (vLLM
        // keeps the generated tokens: `_preempt_request` resets
        // `num_computed_tokens` only)
        let slot_computed = self.p.slot_computed;
        self.sessions[victim].attrs[slot_computed] = computed;
        self.detach(victim);
        // unwind holds inner to `hi` (nested holds), then `hi` itself
        while self.sessions[victim].holds.len() > hi {
            let h = self.sessions[victim].holds.pop().unwrap();
            self.release_hold(victim, &h);
            // pop frames down to and including that hold's frame
            while let Some(f) = self.sessions[victim].frames.pop() {
                if f.kind == FrameKind::Hold && f.block == h.body {
                    break;
                }
            }
        }
        let h_pools: Vec<(usize, f64, &'p CExpr, Option<&'p CExpr>)> = {
            // re-evaluate the hold's requested units from the statement
            let parent = self.sessions[victim]
                .frames
                .last()
                .cloned()
                .expect("hold has a parent frame");
            let p = self.p;
            let stmt = &p.blocks[parent.block][parent.pc - 1];
            let CStmt::Hold { pools, .. } = stmt else {
                panic!("preempted frame is not a hold");
            };
            let mut v = vec![];
            for (r, e, f) in pools {
                let q = self.session_index(r, victim);
                let what = format!("hold {}", self.p.pools[q].name);
                let u = self.amount(e, victim, &what);
                v.push((q, u, e, f.as_ref()));
            }
            v
        };
        let h_exprs: Vec<&'p CExpr> = h_pools.iter().map(|x| x.2).collect();
        let h_fits: Vec<Option<&'p CExpr>> = h_pools.iter().map(|x| x.3).collect();
        let h_pools: Vec<(usize, f64)> = h_pools.iter().map(|x| (x.0, x.1)).collect();
        let lease_pool = {
            let parent = self.sessions[victim].frames.last().cloned().unwrap();
            let p = self.p;
            match &p.blocks[parent.block][parent.pc - 1] {
                CStmt::Hold {
                    lease: Some((r, t)),
                    ..
                } => Some((self.session_index(r, victim), t)),
                _ => None,
            }
        };
        let (body, cache) = {
            let parent = self.sessions[victim].frames.last().cloned().unwrap();
            let p = self.p;
            match &p.blocks[parent.block][parent.pc - 1] {
                CStmt::Hold {
                    body, cache, reuse, ..
                } => (*body, (cache.as_ref(), reuse.as_ref())),
                _ => unreachable!(),
            }
        };
        let (cache, reuse) = cache;
        self.pools[pl].preemptions += 1;
        self.preempted = true;
        let n = h_pools.len();
        let pending = Pending {
            pools: h_pools,
            exprs: h_exprs,
            reserve: h_fits,
            need: vec![0.0; n],
            reuse,
            cache,
            lease: lease_pool,
            body,
            queued_at: self.now,
        };
        self.enqueue_hold(victim, pending, true);
    }

    // -------------------------------------------------------- stages ----

    fn start_job(
        &mut self,
        st: usize,
        owner: Option<usize>,
        work: f64,
        mode: RunMode,
        growing: Option<usize>,
    ) {
        let id = self.next_job;
        self.next_job += 1;
        let now = self.now;
        if work <= 0.0 {
            // nothing to do: the run completes at once (a decode of zero
            // tokens, a transfer of zero bytes)
            if let Some(sid) = owner {
                self.sessions[sid].status = Status::Ready;
                self.ready.push_back(sid);
            }
            return;
        }
        if matches!(self.stages[st].kind, Kind::Shared { .. }) {
            self.next_job -= 1;
            self.start_flow(vec![st], owner, work);
            return;
        }
        if let Some(sid) = owner {
            self.sessions[sid].status = Status::InStage(st, id);
        }
        let stage = &mut self.stages[st];
        let job = Job {
            owner,
            work,
            mode,
            growing,
            enqueued: now,
            started: None,
        };
        match &mut stage.kind {
            Kind::Fifo { queue, .. } => {
                stage.jobs.insert(id, job);
                queue.push_back(id);
                self.fifo_fill(st);
            }
            Kind::Ps {
                v,
                v_last,
                rate,
                set,
                dirty,
                ..
            } => {
                *v += *rate * (now - *v_last);
                *v_last = now;
                let tag = *v + work;
                let mut job = job;
                job.work = tag;
                job.started = Some(now);
                stage.jobs.insert(id, job);
                set.insert((OrdF64(tag), id));
                *dirty = true;
            }
            Kind::Delay => {
                let mut job = job;
                job.started = Some(now);
                stage.jobs.insert(id, job);
                self.at(
                    now + work,
                    Ev::Finish {
                        stage: st,
                        job: id,
                        epoch: 0,
                    },
                );
            }
            Kind::Shared { .. } => unreachable!("`start_job` hands a shared stage to `start_flow`"),
            Kind::Step { residents, .. } => {
                let mut job = job;
                job.started = Some(now);
                stage.jobs.insert(id, job);
                // residents in order of their sessions' hold admissions
                let key = owner.map_or(u64::MAX, |sid| self.sessions[sid].adm_seq);
                let sessions = &self.sessions;
                let jobs = &stage.jobs;
                let pos = residents
                    .iter()
                    .position(|j| jobs[j].owner.map_or(u64::MAX, |o| sessions[o].adm_seq) > key)
                    .unwrap_or(residents.len());
                residents.insert(pos, id);
            }
        }
    }

    // ------------------------------------------------ shared stages ----

    /// A job on shared stages: it holds every one of `stages` until its work
    /// is done at the rate the program's `share` gives it.
    fn start_flow(&mut self, stages: Vec<usize>, owner: Option<usize>, work: f64) {
        let id = self.next_job;
        self.next_job += 1;
        let now = self.now;
        if work <= 0.0 {
            if let Some(sid) = owner {
                self.sessions[sid].status = Status::Ready;
                self.ready.push_back(sid);
            }
            return;
        }
        if let Some(sid) = owner {
            self.sessions[sid].status = Status::InStage(stages[0], id);
        }
        self.flows_advance();
        for (k, &s) in stages.iter().enumerate() {
            self.stages[s].jobs.insert(
                id,
                Job {
                    // the session is readied once, by the first stage
                    owner: if k == 0 { owner } else { None },
                    work,
                    mode: RunMode::Plain,
                    growing: None,
                    enqueued: now,
                    started: Some(now),
                },
            );
        }
        self.flows_touched.extend(&stages);
        self.flows.insert(
            id,
            Flow {
                stages,
                remaining: work,
                rate: 0.0,
            },
        );
        self.flows_dirty = true;
    }

    /// Bring every flow's remaining work to now, at the rates it had.
    fn flows_advance(&mut self) {
        let dt = self.now - self.flows_last;
        if dt > 0.0 {
            for f in self.flows.values_mut() {
                f.remaining = (f.remaining - f.rate * dt).max(0.0);
            }
        }
        self.flows_last = self.now;
    }

    /// The rates of every flow under the program's `share`, and the earliest
    /// end among them scheduled; an end scheduled before is stale.
    fn flows_reschedule(&mut self) {
        self.flows_dirty = false;
        self.flows_advance();
        let cap = |st: &StageState| match st.kind {
            Kind::Shared { cap } => cap,
            _ => unreachable!("a flow holds shared stages only"),
        };
        // Only the flows that share a stage, however indirectly, with one
        // that started or ended can change rate: the connected component of
        // the touched stages. The others keep theirs, which a recomputation
        // would give them again bit for bit: a component no touched stage
        // reaches has had the same flows since it was last computed, and
        // components share no state.
        let touched = std::mem::take(&mut self.flows_touched);
        let mut in_comp = vec![false; self.stages.len()];
        let mut stack: Vec<usize> = vec![];
        for s in touched {
            if !in_comp[s] {
                in_comp[s] = true;
                stack.push(s);
            }
        }
        let mut comp_stages: Vec<usize> = vec![];
        let mut ids: Vec<u64> = vec![];
        while let Some(s) = stack.pop() {
            comp_stages.push(s);
            for id in self.stages[s].jobs.keys() {
                ids.push(*id);
                for &t in &self.flows[id].stages {
                    if !in_comp[t] {
                        in_comp[t] = true;
                        stack.push(t);
                    }
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        match self.p.share.expect("a program with flows has a share") {
            crate::ir::Share::Bottleneck => {
                // each flow's equal share at the tightest of its stages
                for &id in &ids {
                    let r = self.flows[&id]
                        .stages
                        .iter()
                        .map(|&s| cap(&self.stages[s]) / self.stages[s].jobs.len() as f64)
                        .fold(f64::INFINITY, f64::min);
                    self.flows.get_mut(&id).unwrap().rate = r;
                }
            }
            crate::ir::Share::MaxMin => {
                // progressive filling: the stage whose free capacity, split
                // among its flows still rising, is least fills first; its
                // flows stop there. Each stage lists its flows in id order,
                // so a flow is frozen once and the subtractions from a
                // stage's capacity come in the same order as a pass over
                // all flows would make them.
                let n = self.stages.len();
                let mut left = vec![0.0f64; n];
                let mut rising = vec![0usize; n];
                let mut on: Vec<Vec<usize>> = vec![vec![]; n];
                let mut frozen = vec![false; ids.len()];
                for &s in &comp_stages {
                    left[s] = cap(&self.stages[s]);
                }
                for (k, id) in ids.iter().enumerate() {
                    for &s in &self.flows[id].stages {
                        rising[s] += 1;
                        on[s].push(k);
                    }
                }
                let mut open = ids.len();
                while open > 0 {
                    let (s_min, share) = comp_stages
                        .iter()
                        .filter(|&&s| rising[s] > 0)
                        .map(|&s| (s, left[s] / rising[s] as f64))
                        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
                        .expect("an open flow holds a stage");
                    let share = share.max(0.0);
                    for &k in &on[s_min] {
                        if frozen[k] {
                            continue;
                        }
                        frozen[k] = true;
                        open -= 1;
                        let f = self.flows.get_mut(&ids[k]).unwrap();
                        f.rate = share;
                        for &s in &f.stages {
                            left[s] -= share;
                            rising[s] -= 1;
                        }
                    }
                }
            }
        }
        self.flow_epoch += 1;
        let now = self.now;
        let next = self
            .flows
            .iter()
            .filter(|(_, f)| f.rate > 0.0)
            .map(|(id, f)| (now + f.remaining / f.rate, *id))
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        if let Some((t, id)) = next {
            self.at(
                t,
                Ev::Finish {
                    stage: self.flows[&id].stages[0],
                    job: id,
                    epoch: self.flow_epoch,
                },
            );
        }
    }

    /// A flow's work is done: it leaves all its stages, counted at each.
    fn flow_done(&mut self, id: u64) {
        self.flows_advance();
        let Some(f) = self.flows.remove(&id) else {
            return;
        };
        self.flows_touched.extend(&f.stages);
        for s in f.stages {
            if let Some(job) = self.stages[s].jobs.remove(&id) {
                self.job_done(s, job);
            }
        }
        self.flows_dirty = true;
    }

    fn fifo_fill(&mut self, st: usize) {
        let now = self.now;
        let stage = &mut self.stages[st];
        let Kind::Fifo {
            servers,
            queue,
            active,
        } = &mut stage.kind
        else {
            return;
        };
        let mut to_schedule = vec![];
        while active.len() < *servers {
            let Some(id) = queue.pop_front() else { break };
            let j = stage.jobs.get_mut(&id).unwrap();
            j.started = Some(now);
            active.push(id);
            to_schedule.push((now + j.work, id));
        }
        for (t, id) in to_schedule {
            self.at(
                t,
                Ev::Finish {
                    stage: st,
                    job: id,
                    epoch: 0,
                },
            );
        }
    }

    fn ps_phi(&mut self, st: usize, n: usize) -> f64 {
        let p = self.p;
        let CStageKind::Ps(phi) = &p.stages[st].kind else {
            unreachable!()
        };
        let ctx = Ctx {
            n: n as f64,
            ..Default::default()
        };
        self.eval(phi, &ctx, Which::Session)
    }

    fn ps_reschedule(&mut self, st: usize) {
        let now = self.now;
        let n = match &self.stages[st].kind {
            Kind::Ps { set, .. } => set.len(),
            _ => unreachable!(),
        };
        let phi = if n > 0 { self.ps_phi(st, n) } else { 0.0 };
        let Kind::Ps {
            v,
            v_last,
            rate,
            set,
            epoch,
            dirty,
        } = &mut self.stages[st].kind
        else {
            unreachable!()
        };
        *v += *rate * (now - *v_last);
        *v_last = now;
        *rate = if n > 0 { phi / n as f64 } else { 0.0 };
        *epoch += 1;
        *dirty = false;
        let g = *epoch;
        if let Some(&(OrdF64(tag), id)) = set.iter().next()
            && *rate > 0.0
        {
            let dt = ((tag - *v) / *rate).max(0.0);
            self.at(
                now + dt,
                Ev::Finish {
                    stage: st,
                    job: id,
                    epoch: g,
                },
            );
        }
    }

    fn remove_job(&mut self, st: usize, id: u64) {
        if matches!(self.stages[st].kind, Kind::Shared { .. }) {
            // a flow leaves every stage it holds, and the rest re-share
            self.flows_advance();
            if let Some(f) = self.flows.remove(&id) {
                self.flows_touched.extend(&f.stages);
                for s in f.stages {
                    self.stages[s].jobs.remove(&id);
                }
                self.flows_dirty = true;
            }
            return;
        }
        let _now = self.now;
        let stage = &mut self.stages[st];
        let Some(job) = stage.jobs.remove(&id) else {
            return;
        };
        match &mut stage.kind {
            Kind::Fifo { queue, active, .. } => {
                if let Some(i) = active.iter().position(|&j| j == id) {
                    active.remove(i);
                    self.fifo_fill(st);
                } else {
                    queue.retain(|&j| j != id);
                }
            }
            Kind::Ps { set, dirty, .. } => {
                set.remove(&(OrdF64(job.work), id));
                *dirty = true;
            }
            Kind::Delay => {}
            Kind::Shared { .. } => unreachable!("a flow leaves by `remove_job`'s first branch"),
            Kind::Step {
                residents, iter, ..
            } => {
                if let Some(i) = residents.iter().position(|&j| j == id) {
                    residents.remove(i);
                }
                if let Some(it) = iter {
                    it.assign.retain(|&(j, _)| j != id);
                }
            }
        }
    }

    fn on_finish(&mut self, st: usize, id: u64, epoch: u64) {
        let now = self.now;
        // staleness
        match &self.stages[st].kind {
            Kind::Ps { epoch: g, .. } if *g != epoch => return,
            Kind::Shared { .. } => {
                if epoch == self.flow_epoch {
                    self.flow_done(id);
                }
                return;
            }
            _ => {}
        }
        if !self.stages[st].jobs.contains_key(&id) {
            return;
        }
        let stage = &mut self.stages[st];
        let job = stage.jobs.remove(&id).unwrap();
        match &mut stage.kind {
            Kind::Fifo { active, .. } => {
                active.retain(|&j| j != id);
            }
            Kind::Ps {
                v,
                v_last,
                rate,
                set,
                dirty,
                ..
            } => {
                *v += *rate * (now - *v_last);
                *v_last = now;
                set.remove(&(OrdF64(job.work), id));
                *v = v.max(job.work);
                *dirty = true;
            }
            Kind::Delay => {}
            Kind::Shared { .. } => unreachable!("a flow finishes by `flow_done`"),
            Kind::Step { .. } => unreachable!("step jobs finish at iteration ends"),
        }
        self.job_done(st, job);
        if matches!(self.stages[st].kind, Kind::Fifo { .. }) {
            self.fifo_fill(st);
        }
    }

    fn job_done(&mut self, st: usize, job: Job) {
        let now = self.now;
        let stage = &mut self.stages[st];
        let started = job.started.unwrap_or(job.enqueued);
        stage
            .est
            .observe(started, started - job.enqueued, now - started);
        if self.warm {
            stage.completed += 1;
            stage.wait.push(started - job.enqueued);
            stage.service.push(now - started);
        }
        if let Some(sid) = job.owner {
            self.sessions[sid].status = Status::Ready;
            self.ready.push_back(sid);
        }
    }

    // ---------------------------------------------------- step stage ----

    /// The batch as the budget sees it: every resident, decodes taking one
    /// token each. Returns the budget, the chunk and the tokens the
    /// residents want.
    fn pre_iteration(&mut self, st: usize, spec: &CStep) -> (f64, f64, f64) {
        let residents0 = self.residents(st);
        // what `budget` and `chunk` see (`Moment::Budget`): the totals before
        // either expression is read, so `chunk` is read once with them
        let pre = self.resident_totals(st, spec.memory);
        let budget = self.eval(&spec.budget, &pre, Which::Session);
        let chunk = self.eval(&spec.chunk, &pre, Which::Session);
        let mut want = 0.0;
        for &j in &residents0 {
            let job = &self.stages[st].jobs[&j];
            want += if job.mode == RunMode::Decode {
                1.0f64.min(job.work)
            } else if chunk > 0.0 {
                job.work.min(chunk)
            } else {
                job.work
            };
        }
        (budget, chunk, want)
    }

    /// The residents as they stand: how many, how many decode, and the
    /// memory each kind holds in the stage's `memory` pool (`residents`,
    /// `decoders`, `kv_decode`, `kv_prefill`).
    fn resident_totals(&self, st: usize, memory: Option<usize>) -> Ctx {
        let residents = self.residents(st);
        let mut t = Ctx {
            nres: residents.len() as f64,
            ..Default::default()
        };
        for &j in &residents {
            let job = &self.stages[st].jobs[&j];
            let mem = match (memory, job.owner) {
                (Some(pl), Some(sid)) => self.held_in(sid, pl),
                _ => 0.0,
            };
            if job.mode == RunMode::Decode {
                t.ndec += 1.0;
                t.kvb += mem;
            } else {
                t.kvp += mem;
            }
        }
        t
    }

    fn start_iteration(&mut self, st: usize) {
        let p = self.p;
        let CStageKind::Step(spec) = &p.stages[st].kind else {
            unreachable!()
        };
        let (budget, chunk, _want) = self.pre_iteration(st, spec);
        if self.error.is_some() {
            return;
        }
        let mut left = budget;
        let mut assign: Vec<(u64, f64)> = vec![];
        // Every resident is considered once per iteration, in the serving
        // order as it stands when it is its turn. The order is re-read after
        // each one because a growth may have preempted a resident and an
        // admission may have added one, and under `serve by` a newcomer can
        // sort ahead of residents already served: the set of the served, not
        // an index into the list, is what says who is next.
        let mut served: BTreeSet<u64> = BTreeSet::new();
        let exclusive = matches!(spec.serve, CServe::ExclusivePrefill);
        let resident_prefill = self
            .residents(st)
            .iter()
            .any(|&j| self.stages[st].jobs[&j].mode == RunMode::Prefill);
        self.preempted = false;
        let mut attn = 0.0;
        loop {
            let residents = self.serving_order(st, spec);
            let Some(id) = residents.iter().copied().find(|j| !served.contains(j)) else {
                // the running requests are served; admit waiting ones with
                // the budget left, unless this iteration preempted
                // (scheduler.py:869, `if not preempted_reqs`)
                let preempted = self.preempted;
                // A local prefill admitted after tentative decodes replaces
                // them and uses the whole budget (RBLN guard D). Its hold
                // must therefore see that budget, not the decode remainder.
                let admit_left = if exclusive { budget } else { left };
                if left > 0.0 && !preempted && self.admit_bound(st, admit_left) {
                    continue;
                }
                break;
            };
            served.insert(id);
            if let Some(only) = &spec.only {
                if !self.serves(st, id, only, spec.memory) {
                    continue;
                }
            }
            let (mode, remaining, growing, owner) = {
                let j = &self.stages[st].jobs[&id];
                (j.mode, j.work, j.growing, j.owner)
            };
            let want = match mode {
                RunMode::Decode => 1.0f64.min(remaining),
                RunMode::Prefill => {
                    if chunk > 0.0 {
                        remaining.min(chunk)
                    } else {
                        remaining
                    }
                }
                RunMode::Plain => unreachable!(),
            };
            let blocked = exclusive && resident_prefill && mode == RunMode::Decode;
            let available = if exclusive && mode == RunMode::Prefill {
                budget
            } else {
                left
            };
            let tokens = if blocked { 0.0 } else { want.min(available) };
            if tokens <= 0.0 {
                continue;
            }
            // growth before the tokens are committed: the hold must cover
            // the sequence position after this iteration (vLLM
            // `allocate_slots`), block by block
            if let (Some(pl), Some(sid)) = (growing, owner) {
                if matches!(self.sessions[sid].status, Status::Growing(..)) {
                    // stalled from an earlier iteration: no tokens
                    continue;
                }
                let (alloc, pos) = self.hold_alloc_pos(sid, pl);
                if self.error.is_some() {
                    return;
                }
                let need = pos + tokens - alloc;
                if need > EPS && !self.grow(sid, pl, need) {
                    // preempted (lifo): no longer a resident; waiting
                    // (none): stalls as a resident. Either way, no tokens
                    continue;
                }
                if mode == RunMode::Prefill {
                    attn += tokens * (pos + tokens / 2.0);
                }
                // Exclusive-prefill decodes are candidates until waiting
                // admission has finished: replacing them must not publish
                // computed KV for work that will never run.
                if !exclusive {
                    self.advance_pos(sid, pl, tokens);
                }
            } else if mode == RunMode::Prefill {
                attn += tokens * tokens / 2.0;
            }
            if exclusive && mode == RunMode::Prefill {
                // Keep any allocation made for displaced decodes, as RBLN
                // keeps pending runner block deltas; cancel only their work.
                assign.clear();
                left = budget;
            }
            assign.push((id, tokens));
            left -= tokens;
            if left <= 0.0 || (exclusive && mode == RunMode::Prefill) {
                // A selected prefill is a lone batch. In particular do not
                // admit another waiting request with its leftover budget.
                break;
            }
        }
        if exclusive {
            // Growth can preempt an earlier candidate. Only surviving,
            // selected jobs advance their holds' computed positions.
            assign.retain(|(id, _)| self.stages[st].jobs.contains_key(id));
            for &(id, tokens) in &assign {
                let j = &self.stages[st].jobs[&id];
                if let (Some(pl), Some(sid)) = (j.growing, j.owner) {
                    self.advance_pos(sid, pl, tokens);
                }
            }
        }
        // An iteration that scheduled nothing is no iteration, unless it
        // preempted: then it is the scheduler step that only preempted (vLLM's
        // `schedule()` returns with `preempted_reqs` and admits nothing,
        // scheduler.py:869; the oracle driver counts the step; the Lean
        // model's `startIteration` gives that step its cost and `step`
        // re-admits at the next event), and the next iteration re-admits the
        // victim. Dropping it left the engine idle with the victim queued and
        // no event to wake it.
        let preempted = self.preempted;
        if assign.is_empty() && !preempted {
            return;
        }
        let ntok: f64 = assign.iter().map(|a| a.1).sum();
        let mut ndec = 0.0;
        let mut npre = 0.0;
        let mut kvb = 0.0;
        let mut kvp = 0.0;
        for &(id, t) in &assign {
            let j = &self.stages[st].jobs[&id];
            let mem = match (spec.memory, j.owner) {
                (Some(pl), Some(sid)) => self.held_in(sid, pl),
                _ => 0.0,
            };
            match j.mode {
                RunMode::Decode => {
                    ndec += 1.0;
                    kvb += mem;
                }
                _ => {
                    npre += t;
                    kvp += mem;
                }
            }
        }
        let nres = self.residents(st).len() as f64;
        let ctx = Ctx {
            ntok,
            ndec,
            npre,
            nres,
            kvb,
            kvp,
            attn,
            ..Default::default()
        };
        let cost = self.eval(&spec.cost, &ctx, Which::Session).max(0.0);
        // An iteration that schedules tokens lasts a positive time; only the
        // step that merely preempted may cost 0 (`docs/language.md` §3).
        if ntok > 0.0 && cost <= 0.0 {
            self.error = Some(format!(
                "stage `{}`: an iteration of {ntok} tokens costs {cost}; an iteration that \
                 schedules tokens lasts a positive time",
                p.stages[st].name
            ));
            return;
        }
        if self.trace_iter {
            let parts: Vec<String> = assign
                .iter()
                .map(|&(id, t)| {
                    let j = &self.stages[st].jobs[&id];
                    let who = j.owner.map_or(u64::MAX, |sid| self.sessions[sid].serial);
                    let tn = j
                        .owner
                        .map_or(0.0, |sid| self.sessions[sid].attrs[self.p.slot_turn]);
                    format!(
                        "{who}:{tn}:{}{t}",
                        if j.mode == RunMode::Decode { "d" } else { "p" }
                    )
                })
                .collect();
            // in the pool's blocks, or units when it has none
            let kv = spec.memory.map_or((0.0, 0.0), |pl| {
                let b = self.pools[pl].block.unwrap_or(1.0);
                (self.pools[pl].used / b, self.pools[pl].cached / b)
            });
            eprintln!(
                "ITER {:.4} {} | used {} cached {}",
                self.now,
                parts.join(" "),
                kv.0,
                kv.1
            );
        }
        let Kind::Step { iter, epoch, .. } = &mut self.stages[st].kind else {
            unreachable!()
        };
        *epoch += 1;
        let g = *epoch;
        *iter = Some(Iter { epoch: g, assign });
        self.stages[st].iterations += 1;
        let now = self.now;
        let warm = self.warm;
        let steps = &mut self.stages[st].steps;
        steps.set(now, ndec, npre);
        if warm && ndec > 0.0 {
            steps.batch.push(ndec);
            steps.duration.push(cost);
        }
        self.at(
            now + cost,
            Ev::IterEnd {
                stage: st,
                epoch: g,
            },
        );
    }

    /// Whether a queue served by stage `st` has a waiting session.
    fn bound_waiting(&self, st: usize) -> bool {
        self.p
            .pools
            .iter()
            .zip(&self.pools)
            .any(|(cp, pl)| cp.admit_via == Some(st) && !pl.queue.is_empty())
    }

    /// The stage's scheduler admits the selected waiting hold, with
    /// `left` tokens of this iteration's budget left (vLLM's waiting loop,
    /// scheduler.py:868-1128): the units are evaluated now, with
    /// `budget_left(stage) = left`; the first that does not fit stops it.
    /// Returns whether a session was admitted.
    fn admit_bound(&mut self, st: usize, left: f64) -> bool {
        for pl in 0..self.pools.len() {
            if self.p.pools[pl].admit_via != Some(st) {
                continue;
            }
            self.admit_budget = Some((st, left));
            let Some((index, sid)) = self.next_waiter(pl) else {
                self.admit_budget = None;
                continue;
            };
            let pending = self.pending_now(sid);
            self.admit_budget = None;
            let reserve = pending
                .pools
                .iter()
                .zip(&pending.need)
                .all(|(&(q, _), &need)| self.fits(q, self.round_up(q, need)));
            if !reserve {
                return false;
            }
            self.pools[pl].queue.remove(index);
            self.admit(sid, pending);
            // run the admitted session's commands (zero time) until it
            // joins the stage or blocks
            while let Some(s) = self.ready.pop_front() {
                if self.sessions[s].status == Status::Ready {
                    self.exec(s);
                }
            }
            return true;
        }
        false
    }

    /// Residents in the order the iteration serves them: admission order, or
    /// ascending `serve by` keys evaluated per resident (`decoding`,
    /// `admission`, `remaining`, and the residents' totals as they stand,
    /// an admission or a preemption earlier in the iteration included),
    /// ties in admission order (a stable sort of the admission-ordered
    /// list; no keys is that list). `ExclusivePrefill` keeps admission order
    /// and stalls the decodes in the loop instead.
    fn serving_order(&mut self, st: usize, spec: &CStep) -> Vec<u64> {
        let mut r = self.residents(st);
        let CServe::By(keys) = &spec.serve else {
            return r;
        };
        if keys.is_empty() {
            return r;
        }
        let totals = self.resident_totals(st, spec.memory);
        let mut keyed: Vec<(Vec<f64>, u64)> = r
            .drain(..)
            .map(|id| {
                let ctx = self.resident_ctx(st, id, &totals);
                let k = keys
                    .iter()
                    .map(|e| self.eval(e, &ctx, Which::Session))
                    .collect();
                (k, id)
            })
            .collect();
        keyed.sort_by(|a, b| {
            a.0.iter()
                .zip(&b.0)
                .map(|(x, y)| x.total_cmp(y))
                .find(|o| *o != Ordering::Equal)
                .unwrap_or(Ordering::Equal)
        });
        keyed.into_iter().map(|(_, id)| id).collect()
    }

    /// What a serve key or `only` reads for one resident (`Moment::Serve`):
    /// the residents' `totals` and the resident's own.
    fn resident_ctx(&self, st: usize, id: u64, totals: &Ctx) -> Ctx {
        let j = &self.stages[st].jobs[&id];
        Ctx {
            sid: j.owner,
            decoding: (j.mode == RunMode::Decode) as u8 as f64,
            admission: j.owner.map_or(0.0, |s| self.sessions[s].adm_seq as f64),
            remaining: j.work,
            ..totals.clone()
        }
    }

    /// Whether `serve only (expr)` serves resident `id` this iteration, read
    /// on the residents as they stand.
    fn serves(&mut self, st: usize, id: u64, only: &CExpr, memory: Option<usize>) -> bool {
        let totals = self.resident_totals(st, memory);
        let ctx = self.resident_ctx(st, id, &totals);
        self.eval(only, &ctx, Which::Session) != 0.0
    }

    fn residents(&self, st: usize) -> Vec<u64> {
        match &self.stages[st].kind {
            Kind::Step { residents, .. } => residents.clone(),
            _ => unreachable!(),
        }
    }

    fn on_iter_end(&mut self, st: usize, epoch: u64) {
        let Kind::Step { iter, .. } = &mut self.stages[st].kind else {
            unreachable!()
        };
        let Some(it) = iter.take() else { return };
        if it.epoch != epoch {
            *iter = Some(it);
            return;
        }
        let now = self.now;
        self.stages[st].steps.set(now, 0.0, 0.0);
        let mut finished = vec![];
        // the requests that commit a token now: a decode's, or a prefill's
        // at its end
        let mut tokens_of = vec![];
        for (id, tokens) in it.assign {
            if let Some(j) = self.stages[st].jobs.get_mut(&id) {
                j.work -= tokens;
                let done = j.work <= EPS;
                if let Some(sid) = j.owner {
                    match j.mode {
                        RunMode::Decode => tokens_of.push((sid, true)),
                        RunMode::Prefill if done => tokens_of.push((sid, false)),
                        _ => {}
                    }
                }
                if done {
                    finished.push(id);
                }
            }
        }
        for (sid, decode) in tokens_of {
            self.token(st, sid, decode);
        }
        for id in finished {
            let job = self.stages[st].jobs.remove(&id).unwrap();
            let Kind::Step { residents, .. } = &mut self.stages[st].kind else {
                unreachable!()
            };
            if let Some(i) = residents.iter().position(|&j| j == id) {
                residents.remove(i);
            }
            self.job_done(st, job);
        }
    }

    /// A token of `sid`'s turn committed on stage `st` now: a decode's, or
    /// a prefill's at its end. The gap since the turn's previous token goes
    /// to the stage's ITL, wherever that token was. A prefill's end is the
    /// next token after a decode, or after a preemption on the stage of the
    /// previous token (a resumed request samples one); otherwise it is the
    /// client's first, and replaces any before it: a decoder recomputing
    /// the token a prefiller sampled and dropped (llmd_nixl_pull.sq), again
    /// if it is preempted before it ends. The turn's gaps add up to its last
    /// token less its first.
    fn token(&mut self, st: usize, sid: usize, decode: bool) {
        let now = self.now;
        let turn = self.sessions[sid].attrs[self.p.slot_turn];
        let last = self.sessions[sid].last_token.filter(|t| t.turn == turn);
        let decoded = last.is_some_and(|t| t.decoded);
        let next = decode || decoded || last.is_some_and(|t| t.preempted && t.stage == st);
        if let Some(t) = last
            && next
            && self.warm
        {
            self.stages[st].steps.itl.push(now - t.at);
        }
        self.sessions[sid].last_token = Some(LastToken {
            turn,
            at: now,
            stage: st,
            decoded: decoded || decode,
            preempted: false,
        });
    }

    // ---------------------------------------------------- evaluation ----

    fn eval(&mut self, e: &CExpr, ctx: &Ctx, w: Which) -> f64 {
        match e {
            CExpr::Num(x) => *x,
            CExpr::Attr(i) => match (&ctx.snap, ctx.sid) {
                (Some(s), _) => s[*i],
                (None, Some(sid)) => self.sessions[sid].attrs[*i],
                (None, None) => f64::NAN,
            },
            CExpr::Ctx(v) => match v {
                CtxVar::Now => self.now,
                CtxVar::Waited => ctx.waited,
                CtxVar::Size => ctx.size,
                CtxVar::Age => ctx.age,
                CtxVar::Last => ctx.last,
                CtxVar::Queued => ctx.queued,
                CtxVar::N => ctx.n,
                CtxVar::Ntok => ctx.ntok,
                CtxVar::Ndec => ctx.ndec,
                CtxVar::Npre => ctx.npre,
                CtxVar::Nres => ctx.nres,
                CtxVar::Kvb => ctx.kvb,
                CtxVar::Kvp => ctx.kvp,
                CtxVar::Attn => ctx.attn,
                CtxVar::Decoding => ctx.decoding,
                CtxVar::Admission => ctx.admission,
                CtxVar::Remaining => ctx.remaining,
            },
            CExpr::Sample(kind, args) => {
                let a: Vec<f64> = args.iter().map(|x| self.eval(x, ctx, w)).collect();
                // a draw made for a session reads that session's stream; one
                // the machine makes (a cost, a budget, an eviction key) reads
                // the interpreter's
                let rng = match (w, ctx.sid) {
                    (Which::Workload, Some(sid)) => &mut self.sessions[sid].rng_wl,
                    (Which::Session, Some(sid)) => &mut self.sessions[sid].rng,
                    _ => self.rng(w),
                };
                let d = match kind {
                    DistKind::Det => Dist::Deterministic(a[0]),
                    DistKind::Exp => Dist::exp(a[0]),
                    DistKind::Uniform => Dist::Uniform { lo: a[0], hi: a[1] },
                    DistKind::Erlang => Dist::Erlang {
                        k: a[0].max(1.0) as u32,
                        mean: a[1],
                    },
                    DistKind::H2 => Dist::hyperexp_balanced(a[0], a[1].max(1.0)),
                    DistKind::Bernoulli => Dist::bernoulli(a[0]),
                };
                d.sample(rng)
            }
            CExpr::Unary(op, a) => {
                let x = self.eval(a, ctx, w);
                match op {
                    UnOp::Neg => -x,
                    UnOp::Not => {
                        if x != 0.0 {
                            0.0
                        } else {
                            1.0
                        }
                    }
                }
            }
            CExpr::Binary(op, a, b) => {
                let x = self.eval(a, ctx, w);
                // short-circuit
                match op {
                    BinOp::And if x == 0.0 => return 0.0,
                    BinOp::Or if x != 0.0 => return 1.0,
                    _ => {}
                }
                let y = self.eval(b, ctx, w);
                binop(*op, x, y)
            }
            CExpr::Cond(c, a, b) => {
                if self.eval(c, ctx, w) != 0.0 {
                    self.eval(a, ctx, w)
                } else {
                    self.eval(b, ctx, w)
                }
            }
            CExpr::Call(f, args) => self.call(*f, args, ctx, w),
        }
    }

    fn ref_index(&mut self, r: &CRef, ctx: &Ctx, w: Which) -> usize {
        match &r.index {
            None => r.base,
            Some(e) => {
                // a member is a whole number in range; anything else is the
                // program's error, not a member it did not name (#270)
                let x = self.eval(e, ctx, w);
                if !(x >= 0.0 && x.fract() == 0.0 && x < r.count as f64) {
                    if self.error.is_none() {
                        self.error = Some(format!(
                            "index `{}` is {x}: a member of an array of {} is 0 to {}",
                            self.p.show_expr(e),
                            r.count,
                            r.count - 1
                        ));
                    }
                    return r.base;
                }
                r.base + x as usize
            }
        }
    }

    fn call(&mut self, f: Fun, args: &[CArg], ctx: &Ctx, w: Which) -> f64 {
        let num = |s: &mut Self, i: usize| match &args[i] {
            CArg::Expr(e) => s.eval(e, ctx, w),
            _ => f64::NAN,
        };
        let stage = |s: &mut Self, i: usize| match &args[i] {
            CArg::Stage(r) => s.ref_index(r, ctx, w),
            _ => unreachable!("linker checked"),
        };
        let pool = |s: &mut Self, i: usize| match &args[i] {
            CArg::Pool(r) => s.ref_index(r, ctx, w),
            _ => unreachable!("linker checked"),
        };
        match f {
            Fun::Min => num(self, 0).min(num(self, 1)),
            Fun::Max => num(self, 0).max(num(self, 1)),
            Fun::Abs => num(self, 0).abs(),
            Fun::Floor => num(self, 0).floor(),
            Fun::Ceil => num(self, 0).ceil(),
            Fun::Sqrt => num(self, 0).sqrt(),
            Fun::Exp => num(self, 0).exp(),
            Fun::Ln => num(self, 0).ln(),
            Fun::Pow => num(self, 0).powf(num(self, 1)),
            Fun::Queue => {
                let s = stage(self, 0);
                self.stages[s].jobs.len() as f64
            }
            Fun::Busy => {
                let s = stage(self, 0);
                match &self.stages[s].kind {
                    Kind::Fifo { active, .. } => active.len() as f64,
                    Kind::Step { iter, .. } => {
                        iter.as_ref().map_or(0.0, |it| it.assign.len() as f64)
                    }
                    _ => self.stages[s].jobs.len() as f64,
                }
            }
            Fun::Work => {
                let s = stage(self, 0);
                let now = self.now;
                let st = &self.stages[s];
                match &st.kind {
                    Kind::Ps {
                        v, v_last, rate, ..
                    } => {
                        let vv = v + rate * (now - v_last);
                        st.jobs.values().map(|j| (j.work - vv).max(0.0)).sum()
                    }
                    Kind::Fifo { active, .. } => st
                        .jobs
                        .iter()
                        .map(|(id, j)| {
                            if active.contains(id) {
                                (j.work - (now - j.started.unwrap())).max(0.0)
                            } else {
                                j.work
                            }
                        })
                        .sum(),
                    Kind::Delay => st
                        .jobs
                        .values()
                        .map(|j| (j.work - (now - j.started.unwrap())).max(0.0))
                        .sum(),
                    Kind::Shared { .. } => {
                        let dt = now - self.flows_last;
                        let mut ids: Vec<u64> = st.jobs.keys().copied().collect();
                        ids.sort_unstable();
                        ids.iter()
                            .filter_map(|id| self.flows.get(id))
                            .map(|f| (f.remaining - f.rate * dt).max(0.0))
                            .sum()
                    }
                    Kind::Step { .. } => st.jobs.values().map(|j| j.work).sum(),
                }
            }
            Fun::Used => {
                let p = pool(self, 0);
                self.pools[p].used
            }
            Fun::Free => {
                let p = pool(self, 0);
                self.pools[p].cap - self.pools[p].used
            }
            Fun::CachedIn => {
                let p = pool(self, 0);
                match ctx.sid {
                    Some(sid) => self.own_entry_size(p, sid),
                    None => 0.0,
                }
            }
            Fun::Holders => {
                let p = pool(self, 0);
                self.pools[p].holders.len() as f64
            }
            Fun::Queued => {
                let p = pool(self, 0);
                self.pools[p].queue.len() as f64
            }
            Fun::Price => {
                let s = stage(self, 0);
                let sh = num(self, 1);
                let ds = num(self, 2);
                self.stages[s].est.price(sh, ds)
            }
            Fun::BudgetLeft => {
                let s = stage(self, 0);
                if let Some((bs, left)) = self.admit_budget
                    && bs == s
                {
                    return left;
                }
                let p = self.p;
                let CStageKind::Step(spec) = &p.stages[s].kind else {
                    unreachable!("Program::validate refuses budget_left of a non-step stage (#268)")
                };
                let (budget, _, want) = self.pre_iteration(s, spec);
                (budget - want).max(0.0)
            }
            Fun::EstLambda => {
                let s = stage(self, 0);
                self.stages[s].est.estimates().0
            }
            Fun::EstRho => {
                let s = stage(self, 0);
                self.stages[s].est.estimates().1
            }
            Fun::EstWait => {
                let s = stage(self, 0);
                self.stages[s].est.estimates().2
            }
        }
    }

    // -------------------------------------------------------- report ----

    fn debug_pools(&self) {
        for (pl, cp) in self.pools.iter().zip(self.p.pools.iter()) {
            let mut by: std::collections::BTreeMap<String, (usize, f64)> = Default::default();
            for (serial, e) in pl.entries.iter().map(|(k, e)| (k, e.size)) {
                let st = match self.by_serial.get(serial) {
                    None => "gone".to_string(),
                    Some(&sid) => format!("{:?}", self.sessions[sid].status)
                        .split('(')
                        .next()
                        .unwrap()
                        .to_string(),
                };
                let x = by.entry(st).or_default();
                x.0 += 1;
                x.1 += e;
            }
            eprintln!(
                "POOL {} used {} cached {} by status {:?}",
                cp.name, pl.used, pl.cached, by
            );
        }
    }

    fn report(&mut self) -> Report {
        if std::env::var_os("SERQ_DUMP_POOLS").is_some() {
            self.debug_pools();
        }
        let now = self.now;
        let span = now - self.p.warmup;
        let p = self.p;
        let observes = self
            .observes
            .iter()
            .zip(&p.observes)
            .enumerate()
            .map(|(k, (o, name))| ObserveReport {
                name: name.clone(),
                is_test: p.observe_is_test(k),
                count: o.w.count(),
                mean: o.w.mean(),
                cv2: o.w.cv2(),
                ci: if o.samples.len() >= CI_MIN_SAMPLES {
                    batch_means(&o.samples, 20)
                } else {
                    Estimate::nan()
                },
                p99: quantile(&o.samples, 0.99),
                samples: o.samples.clone(),
                records: o.records.clone(),
            })
            .collect();
        let gauges = self
            .gauges
            .iter()
            .zip(&p.gauges)
            .map(|(pts, g)| {
                let ts = time_stats(pts, p.warmup, now, 20);
                GaugeReport {
                    name: g.name.clone(),
                    mean: ts.mean,
                    ci: ts.ci,
                    min: ts.min,
                    max: ts.max,
                    points: pts.clone(),
                }
            })
            .collect();
        let stages = self
            .stages
            .iter()
            .zip(&p.stages)
            .map(|(s, cs)| StageReport {
                name: cs.name.clone(),
                index: cs.index,
                mean_number: s.number_avg.mean(now),
                utilization: s.busy_avg.mean(now),
                completed: s.completed,
                throughput: s.completed as f64 / span,
                mean_wait: s.wait.mean(),
                mean_service: s.service.mean(),
                iterations: s.iterations,
                prefill_only: s.steps.prefill_only.mean(now),
                decode_only: s.steps.decode_only.mean(now),
                mixed: s.steps.mixed.mean(now),
                mean_decodes: s.steps.decodes.mean(now),
                mean_decode_batch: s.steps.batch.mean(),
                mean_decode_step: s.steps.duration.mean(),
                mean_itl: s.steps.itl.mean(),
                itl_p50: s.steps.itl.quantile(0.5),
                itl_p99: s.steps.itl.quantile(0.99),
            })
            .collect();
        let pools = self
            .pools
            .iter()
            .zip(&p.pools)
            .map(|(pl, cp)| PoolReport {
                name: cp.name.clone(),
                index: cp.index,
                mean_used: pl.used_avg.mean(now),
                mean_cached: pl.cached_avg.mean(now),
                mean_queue: pl.queue_avg.mean(now),
                mean_holders: pl.holders_avg.mean(now),
                mean_wait: pl.wait.mean(),
                admissions: pl.admissions,
                evicted_entries: pl.evicted_entries,
                evicted_units: pl.evicted_units,
                preemptions: pl.preemptions,
                spills: pl.spills,
                rejected: pl.rejected,
                stuck: pl.stuck,
            })
            .collect();
        Report {
            horizon: p.horizon,
            end: now,
            warmup: p.warmup,
            seed: p.seed,
            events: self.events,
            arrivals: self.arrivals,
            ended: self.ended,
            turns: self.turns,
            mean_live: self.live_avg.mean(now),
            observes,
            gauges,
            stages,
            pools,
        }
    }
}

/// An eviction key ordered lexicographically by `total_cmp` (the order of
/// `lex_less`).
#[derive(PartialEq)]
struct KeyOrd(Vec<f64>);

impl Eq for KeyOrd {}

impl PartialOrd for KeyOrd {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for KeyOrd {
    fn cmp(&self, o: &Self) -> Ordering {
        for (x, y) in self.0.iter().zip(&o.0) {
            match x.total_cmp(y) {
                Ordering::Equal => {}
                c => return c,
            }
        }
        self.0.len().cmp(&o.0.len())
    }
}

/// Whether an eviction key reads only its entry, the clock and the stage
/// estimates (no pool or queue state, which eviction changes, and no
/// sampling, whose draws would be reordered).
/// A reference's index counts like any other operand: a draw or a pool
/// read in it (`est_wait(E[floor(~uniform(0, 2))])`) makes the key dynamic
/// (#273).
fn static_key(e: &CExpr) -> bool {
    !e.any(&|x| match x {
        CExpr::Sample(..) => true,
        CExpr::Call(f, _) => !matches!(
            f,
            Fun::Min
                | Fun::Max
                | Fun::Abs
                | Fun::Floor
                | Fun::Ceil
                | Fun::Sqrt
                | Fun::Exp
                | Fun::Ln
                | Fun::Pow
                | Fun::Price
                | Fun::EstLambda
                | Fun::EstRho
                | Fun::EstWait
        ),
        _ => false,
    })
}

fn lex_less(a: &[f64], b: &[f64]) -> bool {
    for (x, y) in a.iter().zip(b) {
        match x.total_cmp(y) {
            Ordering::Less => return true,
            Ordering::Greater => return false,
            Ordering::Equal => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key that draws in a reference's index is not static: keying every
    /// entry once would reorder its draws (#273).
    #[test]
    fn a_draw_in_an_index_makes_a_key_dynamic() {
        let est = |index: Option<CExpr>| {
            CExpr::Call(
                Fun::EstWait,
                vec![CArg::Stage(CRef {
                    base: 0,
                    count: 2,
                    index: index.map(Box::new),
                })],
            )
        };
        assert!(static_key(&est(None)));
        assert!(static_key(&est(Some(CExpr::Attr(0)))));
        let draw = CExpr::Sample(DistKind::Uniform, vec![CExpr::Num(0.0), CExpr::Num(2.0)]);
        assert!(!static_key(&est(Some(draw))));
        let read = CExpr::Call(
            Fun::Used,
            vec![CArg::Pool(CRef {
                base: 0,
                count: 1,
                index: None,
            })],
        );
        assert!(!static_key(&est(Some(read))));
    }
}
