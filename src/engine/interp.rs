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
use std::rc::Rc;

use rand::Rng;
use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::engine::cache::{Seg, Trie};
use crate::engine::dist::Dist;
use crate::engine::log::{EventKind, Log, Who};
use crate::engine::report::*;
use crate::engine::state::{Moved, Status as ReqStatus, Transfer};
use crate::engine::stats::*;

use crate::frontend::ast::{BinOp, RunMode, UnOp};
use crate::frontend::link::*;
use crate::ir::Preempt;
use crate::ir::Side;
use crate::ir::trace::Corpus;
use crate::ir::{CConnector, CIter, CRequest, CStageKind, ClaimKind};

/// The tolerance of a comparison of amounts that sums of floats produce
/// (units, tokens, work): below it, two amounts are equal.
const EPS: f64 = 1e-9;

// ------------------------------------------------------------ events ----

enum Ev {
    Arrive,
    Finish {
        stage: usize,
        job: u64,
        epoch: u64,
    },
    /// A run's `latency` is over: its job, at `stage` (the first of a
    /// flow's) since the run began, starts its work. A job gone meanwhile
    /// (its session ended) is not there, and the event passes.
    Start {
        stage: usize,
        job: u64,
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
    /// A tick of a pool under `preempt leases by … every`.
    PoolTick {
        pool: usize,
    },
    /// A tick of a pool under `queue … every`.
    QueueTick {
        pool: usize,
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
impl Entry {
    /// Within an instant, the pools' ticks come after its other events, in
    /// the order the pools are declared; a pool's queue tick before its
    /// preemption tick.
    fn rank(&self) -> (u8, usize, u8) {
        match self.ev {
            Ev::QueueTick { pool } => (1, pool, 0),
            Ev::PoolTick { pool } => (1, pool, 1),
            _ => (0, 0, 0),
        }
    }
}

impl Ord for Entry {
    fn cmp(&self, o: &Self) -> Ordering {
        o.time
            .total_cmp(&self.time)
            .then_with(|| o.rank().cmp(&self.rank()))
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
    /// A request's load waits for its source's send (`CTransfer::source`)
    /// in pool `.0`: a push's leg's prefill, not ended yet.
    AwaitingSend(usize),
    Ended,
}

/// One pool of a hold: what it has allocated there, and where the session
/// stands in its sequence.
#[derive(Clone, Debug)]
struct Held {
    pool: usize,
    alloc: f64,
    /// Advanced by `growing` runs and `load`; starts at the consumed
    /// cached prefix.
    pos: f64,
    /// The units the admission tested (`reserve`, or the units): under the
    /// pool's `reserve held`, what it has not allocated of them counts
    /// against every later admission while the hold lasts.
    reserved: f64,
    /// The cached prefix it reads through: the first `shared` units of its
    /// chain, part of `alloc`, in the nodes `path` of the pool's trie, which
    /// it keeps locked.
    shared: f64,
    path: Vec<usize>,
}

impl Held {
    /// What has been computed, and so may be cached: the position, once a
    /// `growing` run of the hold advanced it, else the whole allocation.
    fn computed(&self, grown: bool) -> f64 {
        if grown { self.pos } else { self.alloc }
    }
}

#[derive(Clone, Debug)]
struct Hold<'p> {
    pools: Vec<Held>,
    /// Whether a `growing` run advanced `pos` (then `pos`, not the
    /// allocation, is what has been computed).
    grown: bool,
    cache: Option<&'p CExpr>,
    chain: Chain,
    /// The pool whose allocation outlives the scope, and for how long.
    lease: Option<(usize, Expiry<'p>)>,
    body: BlockId,
    /// The hold of an engine's request: no body frame; a preemption puts
    /// the request back in the queue, and its end goes on with the request.
    request: bool,
    /// Part of the request's hold: the pools its admission, read again after
    /// its connector's load, takes of what it had taken none (a running
    /// slot, vLLM's waiting request once its remote KV has arrived). No body
    /// frame; it goes with the request's hold.
    part: bool,
}

/// A request to an engine (`CStmt::Request`), as vLLM keeps a `Request`:
/// the tokens it has and what it has generated. Its KV position is its
/// hold's on the engine's memory pool. A preemption keeps all of it but the
/// position (`_preempt_request` resets `num_computed_tokens` only,
/// scheduler.py:1561).
#[derive(Clone, Debug)]
struct Req<'p> {
    stage: usize,
    spec: Option<&'p CRequest>,
    /// The engine's memory pool, whose position is the request's KV.
    memory: Option<usize>,
    input: f64,
    output: f64,
    /// The tokens it has: its input and what it has generated.
    known: f64,
    generated: f64,
    /// `min(cachedin(memory), reuse)`, read at its latest admission.
    hit: f64,
    /// What its connector loads, read at its latest admission after `hit`.
    loaded: f64,
    /// Whether it has been preempted since its call.
    preempted: bool,
    /// The session's frames when a connector's block was pushed: the block
    /// has run when they are as many again.
    depth: usize,
    /// Read by `cache` at a release: the request has generated its output.
    finished: bool,
    /// Whether the first token has been sampled.
    first: bool,
    phase: Phase,
    /// Its number among its session's (or leg's) calls, which names it in
    /// the log with its engine.
    call: u32,
    /// Its status (`engine::state::Status`) and mode, as `restatus` last
    /// read it, and since when.
    status: Option<(ReqStatus, RunMode)>,
    since: f64,
    /// The source's send its load's copy reads (`Lease::id`), chosen when
    /// the copy starts: the one its load ends.
    reads: Option<u64>,
    /// Its admission found the send it would read gone: it loads nothing,
    /// and its load is over when it is admitted (`E.load_done`).
    unread: bool,
}

/// Where a request stands, read when its session is ready again: admitted
/// (or called, taking nothing), its connector loading, its prefill ended,
/// its decode ended, its connector saving.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Admitted,
    Loading,
    /// Loaded, waiting for the pools its admission takes now.
    Waiting,
    Prefill,
    Decode,
    Saving,
}

/// When an allocation kept past its scope ends: after a hold's `lease`
/// expression's seconds, read at its scope's end, or after a send's `ttl`
/// (`CSend::ttl`; none, never).
#[derive(Clone, Copy, Debug)]
enum Expiry<'p> {
    After(&'p CExpr),
    Fixed(f64),
}

/// What a release keeps cached: its `cache` clause, read then, or an
/// amount read before (an engine request's lease, `Lease::keep`).
#[derive(Clone, Copy)]
enum Keep<'p> {
    Clause(&'p CExpr),
    Read(f64),
}

/// An allocation that outlived its scope (`lease P (t)`): the session's
/// until its `release` of the pool, the expiry or its end, then cached per
/// `cache`. Not a hold: no `growing` run. A `grow` outside every hold of
/// the pool grows it, and under `preempt leases by` another growth may end
/// it (`Interp::revoke`).
#[derive(Clone, Debug)]
struct Lease<'p> {
    id: u64,
    pool: usize,
    alloc: f64,
    computed: f64,
    cache: Option<&'p CExpr>,
    /// The hold's chain and the prefix it read through, which the lease
    /// keeps locked until it ends.
    chain: Chain,
    shared: f64,
    path: Vec<usize>,
    /// When it expires (`inf`: only a `release` or the end takes it).
    expires: f64,
    /// The attributes of the leg that leased it, which `cache` reads when
    /// the lease ends: a leg's attributes are its own, and its lease passes
    /// to the session when it ends.
    attrs: Option<Vec<f64>>,
    /// An engine request's lease: what its `cache` keeps, read when the
    /// request ended (`Moment::Release`), not when the lease does, when the
    /// session's request may be another.
    keep: Option<f64>,
    /// Set aside by a tick under `suspend`: its units are the pool's again
    /// and `alloc` is what a later tick gives back.
    suspended: bool,
    /// A source request's send (`CRequest::send`): who made the request, its
    /// call and engine, and since when it is `finished`.
    send: Option<SendOf>,
    /// A reader's copy reads it: its `ttl` no longer ends it, the read does
    /// (vLLM's decoder renews the lease while it reads).
    reading: bool,
}

/// The request a send is of (`Lease::send`).
#[derive(Clone, Copy, Debug)]
struct SendOf {
    who: Who,
    call: u32,
    stage: usize,
    since: f64,
}

/// The allocation a `grow` enlarges: a hold's entry (the hold, the pool's
/// place in it) or a lease (its place in the session's leases).
#[derive(Clone, Copy, Debug)]
enum Grown {
    Hold(usize, usize),
    Lease(usize),
}

/// The run-time error of a `grow` or `shrink` with nothing to act on.
fn nothing_held(what: &str, name: &str) -> String {
    format!(
        "`{what} {name}`: the session holds no `{name}` and leases none; a lease ends at \
         its `release`, its expiry, or under `preempt leases by` (`allocated({name})` is \
         0 then)"
    )
}

/// Whether the session waits to grow in pool `pool`.
fn grows_in(s: &Session, pool: usize) -> bool {
    matches!(s.status, Status::Growing(q, ..) if q == pool)
}

/// Whether the session has an allocation in pool `pool`: a hold of it, or
/// a lease, which frees it when it ends.
fn allocates_in(s: &Session, pool: usize) -> bool {
    s.holds
        .iter()
        .any(|h| h.pools.iter().any(|held| held.pool == pool))
        || s.leases.iter().any(|l| l.pool == pool)
}

/// The sessions among a pool's `holders`, which has an entry per hold and
/// per lease: a session holding the pool twice is one.
fn sessions_in(holders: &[usize]) -> usize {
    let mut v = holders.to_vec();
    v.sort_unstable();
    v.dedup();
    v.len()
}

/// One pool a waiting hold asks for.
#[derive(Clone, Debug)]
struct Wanted<'p> {
    pool: usize,
    units: f64,
    /// The unit expression, re-evaluated at admission (the lecture's
    /// `[Admit]` evaluates `c(x_r)` then: observables such as the cache or
    /// the engine's budget may have changed while the session waited).
    expr: &'p CExpr,
    /// Units that must fit for the admission (default: the allocation),
    /// e.g. the whole prompt while only its first chunk is allocated (vLLM
    /// `scheduler_reserve_full_isl`).
    reserve: Option<&'p CExpr>,
    /// What the admission waits for: the units, or the reservation above
    /// them, as `pending_now` last evaluated them.
    need: f64,
    /// Of `need`, the cached prefix another live hold reads through: it is
    /// allocated already, and the admission does not ask for it again.
    shared: f64,
}

/// A hold's sequence as keyed segments (`prefix`), evaluated when it
/// asks; `(serial, ∞)` without the clause.
type Chain = Rc<[Seg]>;

#[derive(Clone, Debug)]
struct Pending<'p> {
    pools: Vec<Wanted<'p>>,
    reuse: Option<&'p CExpr>,
    cache: Option<&'p CExpr>,
    chain: Chain,
    lease: Option<(usize, Expiry<'p>)>,
    body: BlockId,
    queued_at: f64,
    /// An engine's request's admission (`Hold::request`).
    request: bool,
    /// The pools a request's admission read after its load and had taken
    /// none of (`Hold::part`).
    part: bool,
}

/// A session, or a leg of one (`fork`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Leg {
    /// A session.
    No,
    /// A leg of the session in this slot, the n-th it forked (the log
    /// names a leg by its number: a leg shares its session's serial). The
    /// slot stays until the leg ends: a session may not end before its
    /// legs.
    Of(usize, u32),
    /// A leg of a session that was refused and has ended.
    Orphan(u32),
}

impl Leg {
    /// The leg's number, `None` for a session.
    fn number(self) -> Option<u32> {
        match self {
            Leg::No => None,
            Leg::Of(_, n) | Leg::Orphan(n) => Some(n),
        }
    }
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
    /// Per pool, the chain along which the session or one of its legs last
    /// cached there: the key `cachedin` reads the pool by when no hold of
    /// the session names it (#595: a `prefix` keys only the pools its hold
    /// holds). A leg's stays empty: its session keeps it (`Interp::root`).
    cached_chain: HashMap<usize, Chain>,
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
    /// The turn in progress, as the session's client sees it.
    client: Option<ClientTurn>,
    /// Whether this is a leg (`fork`), and of which session.
    leg: Leg,
    /// Legs forked and not yet ended, and forks so far (the key of a leg's
    /// stream).
    legs: u32,
    forks: u64,
    /// The engine request in progress (`CStmt::Request`).
    req: Option<Req<'p>>,
    /// Engine requests called so far (`Req::call`).
    calls: u32,
}

impl Session<'_> {
    /// The innermost hold of pool `pl`, as (the hold, the pool's place in
    /// it): the one `grow`, `load`, `release` and `growing` act on.
    fn innermost(&self, pl: usize) -> Option<(usize, usize)> {
        self.holds
            .iter()
            .enumerate()
            .rev()
            .find_map(|(hi, h)| h.pools.iter().position(|e| e.pool == pl).map(|k| (hi, k)))
    }
}

/// The stream of one (seed, session, turn, kind): splitmix64 over the four,
/// so that no two sessions, turns or kinds share a stream. The seed is mixed
/// alone first, so that no (seed, serial) pair shares a stream with another
/// seed's session (#297).
fn substream(seed: u64, serial: u64, turn: u64, kind: u64) -> StdRng {
    let mut x = mix(seed);
    for v in [serial, turn, kind] {
        x = mix(x ^ v);
    }
    StdRng::seed_from_u64(x)
}

/// One splitmix64 step.
fn mix(x: u64) -> u64 {
    let mut x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Kinds of a session's streams (`substream`).
const STREAM_WORKLOAD: u64 = 1;
const STREAM_SESSION: u64 = 2;
const STREAM_LEG: u64 = 3;

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

/// A turn as the virtual client that sent it sees it
/// (`docs/design/client-metrics.md`): sent at its `Turn`, answered when
/// the server statements spliced after the `Turn` are done. Its first
/// output token is the prefill's that ended just before its first decode
/// token, or that decode token when no prefill did; its tokens after the
/// first are the ones committed after it.
#[derive(Clone, Copy, Debug)]
struct ClientTurn {
    /// The turn number (`slot_turn`): a leg's tokens count for this turn
    /// only while the leg is of it.
    turn: f64,
    start: f64,
    /// The turn's latest prefill end before its first token: the token it
    /// sampled is the client's first if a decode follows.
    prefill_end: Option<f64>,
    first: Option<f64>,
    /// The output tokens committed after the first.
    after: u64,
    /// The turn is answered when the frame at this depth, on this block,
    /// reaches this statement: the first after the `Turn` on the workload's
    /// side, or the block's end.
    depth: usize,
    block: BlockId,
    end: usize,
}

/// The client's metrics, in the report's order.
pub const CLIENT: [&str; 3] = ["ttft", "tpot", "response"];
const TTFT: usize = 0;
const TPOT: usize = 1;
const RESPONSE: usize = 2;

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
    /// The prefix cache, by keyed segments (a writer may have ended).
    cache: Trie,
    /// Waiting holds in enqueue order; policy keys are never cached.
    queue: VecDeque<WaitingHold>,
    growers: VecDeque<usize>,
    /// The number of the next tick under `preempt leases by … every`, when
    /// one is due.
    next_tick: Option<u64>,
    /// The number of the next tick under `queue … every`, when one is due.
    next_queue_tick: Option<u64>,
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
    /// A `growing` run's pool.
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
    /// flows of `Interp::flows`, their rates set by its fabric's `share`.
    Shared {
        cap: f64,
        share: crate::ir::Share,
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
    /// A step stage whose last try at an iteration scheduled nothing while it
    /// had residents or a waiting queue it serves, and none since: an engine
    /// waiting for an event that may not come (#263).
    idle_with_work: bool,
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

/// An iteration being planned by a step stage's body: what it has served
/// and scheduled so far, and the budget left.
struct Plan<'p> {
    st: usize,
    spec: &'p CStep,
    chunk: f64,
    left: f64,
    served: BTreeSet<u64>,
    assign: Vec<(u64, f64)>,
    attn_by: Vec<(u64, f64)>,
    admitted: f64,
    /// A prefill the `granule` refused: the iteration admits no one after.
    refused: bool,
}

/// `exclusive prefill`'s rule, as `give` applies it: whether a prefill is
/// resident as the iteration starts, and the whole budget a selected
/// prefill takes.
#[derive(Clone, Copy)]
struct Exclusive {
    resident_prefill: bool,
    budget: f64,
}

/// What giving a resident its tokens came to.
enum Give {
    /// No tokens: none wanted, none left, or a stalled grower.
    Skipped,
    /// No tokens: a prefill the `granule` refuses what is left. The
    /// iteration admits no one after it, as TensorRT-LLM's scan stops at
    /// the first context that does not fit (`microBatchScheduler.cpp`
    /// L428-L431 at bf414e37).
    Refused,
    /// Its tokens, in this mode.
    Gave(RunMode),
    /// It preempted itself, or the run failed: serving stops here.
    Stopped,
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
    position: f64,
    admitted: f64,
    preempted: f64,
    demand: f64,
    served: f64,
    arrived: f64,
}

impl Ctx {
    fn session(sid: usize) -> Self {
        Ctx {
            sid: Some(sid),
            ..Default::default()
        }
    }
}

/// What the run found of one claim so far.
#[derive(Default)]
struct ClaimState {
    /// Iterations the claim was read at.
    checked: u64,
    /// Of those, the ones it read as 0.
    failures: u64,
    /// An `every` claim's first failure, a `some` claim's first witness.
    first: Option<f64>,
    /// The serial of the first session that failed its `given`: the claim
    /// is out of scope for the run, and read no further.
    out_of_scope: Option<u64>,
}

struct ObserveStat {
    w: Welford,
    samples: Vec<f64>,
    /// (time, session serial, turn number) of every sample
    records: Vec<(f64, u64, u32)>,
}

impl ObserveStat {
    fn new() -> Self {
        ObserveStat {
            w: Welford::new(),
            samples: vec![],
            records: vec![],
        }
    }

    /// The report of the observation `name`, a program's or the client's.
    fn report(&self, name: &str, is_test: bool) -> ObserveReport {
        ObserveReport {
            name: name.to_string(),
            is_test,
            count: self.w.count(),
            mean: self.w.mean(),
            cv2: self.w.cv2(),
            ci: if self.samples.len() >= CI_MIN_SAMPLES {
                batch_means(&self.samples, 20)
            } else {
                Estimate::nan()
            },
            p99: quantile(&self.samples, 0.99),
            samples: self.samples.clone(),
            records: self.records.clone(),
        }
    }
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
    /// The client's `ttft`, `tpot` and `response` (`CLIENT`).
    client: Vec<ObserveStat>,
    /// Per gauge: its change points `(time, value)`, the value from that
    /// time on, one point per instant (the state after the instant's last
    /// event).
    gauges: Vec<Vec<(f64, f64)>>,
    /// Per claim: what the run found of it.
    claims: Vec<ClaimState>,
    /// Per stage: the tokens its iterations scheduled so far, the run's
    /// whole (an iteration claim's `served`).
    served: Vec<f64>,
    /// Per observation a claim `at end` aggregates: every value observed,
    /// warm-up included.
    observed: Vec<Option<Vec<f64>>>,
    /// Per pool: its eviction keys do not change while other entries are
    /// evicted (no pool or stage queries, no sampling), so `make_room` can
    /// key every entry once.
    evict_static: Vec<bool>,
    /// Debug switch (`SERQ_TRACE_EVICT`), read once. An iteration's batch
    /// and memory are in the log (`EventKind::Iter`).
    trace_evict: bool,
    live: usize,
    live_avg: TimeAverage,
    /// While a stage admits from a queue it serves: (stage, budget left).
    admit_budget: Option<(usize, f64)>,
    next_dead: u64,
    next_lease: u64,
    /// Whether a preemption happened since the iteration being scheduled
    /// began (vLLM's `preempted_reqs`, scheduler.py:869): set by `preempt`,
    /// cleared where `start_iteration` begins to serve its residents.
    preempted: bool,
    /// How many residents the iteration being scheduled has preempted so far
    /// (`preempted` in an iteration body).
    preempted_now: f64,
    /// The step stages' registers (`state`), as their iteration bodies last
    /// set them.
    regs: Vec<f64>,
    next_adm: u64,
    next_release: u64,
    arrivals: u64,
    ended: u64,
    turns: u64,
    events: u64,
    /// The log of the run (`engine::log`), when asked for (`logged`).
    log: Option<Log>,
    /// Per engine request status (`ReqStatus::ALL`): the requests in it,
    /// time-averaged, and its stays that ended after warm-up.
    status_avg: Vec<TimeAverage>,
    stays: Vec<LogHistogram>,
}

/// Tags of dead cache nodes (units past a `reuse` bound) start here, above
/// every session serial.
const DEAD_ENTRY: u64 = 1 << 62;

/// What a segment's key names: the attribute it reads, or, for any other
/// expression, the expression. Two keys are one when they name the same
/// thing and have the same value: `prefix (system: S)` with `system = 3`
/// is not the entry of the session whose serial is 3.
fn key_space(k: &CExpr) -> u64 {
    match k {
        CExpr::Attr(a) => *a as u64,
        _ => {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            format!("{k:?}").hash(&mut h);
            h.finish() | 1 << 63
        }
    }
}

/// A chain of no `prefix`: the session's own, `(serial: ∞)`.
fn own_chain(p: &crate::ir::Program, serial: u64) -> Chain {
    Rc::from([(p.slot_serial as u64, serial as f64, f64::INFINITY)])
}

/// Which pool a `hold` waits at: the first one listed.
fn first_pool(pending: &Pending) -> usize {
    pending.pools[0].pool
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
                cache: Trie::new(p.slot_serial as u64),
                queue: VecDeque::new(),
                growers: VecDeque::new(),
                next_tick: None,
                next_queue_tick: None,
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
        let shared = p
            .stage_shares()
            .expect("a linked program's fabrics have one policy each");
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
                    CStageKind::Ps(CExpr::Num(cap)) if let Some(share) = shared[k] => {
                        Kind::Shared { cap: *cap, share }
                    }
                    // `ps(present)` gives every job rate 1: a finish event
                    // each, no virtual time to re-share at every arrival
                    _ if cs.kind.is_delay() => Kind::Delay,
                    CStageKind::Ps(_) => Kind::Ps {
                        v: 0.0,
                        v_last: 0.0,
                        rate: 0.0,
                        set: BTreeSet::new(),
                        epoch: 0,
                        dirty: false,
                    },
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
                idle_with_work: false,
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
            observes: p.observes.iter().map(|_| ObserveStat::new()).collect(),
            client: CLIENT.iter().map(|_| ObserveStat::new()).collect(),
            gauges: p.gauges.iter().map(|_| vec![]).collect(),
            claims: p.claims.iter().map(|_| ClaimState::default()).collect(),
            served: vec![0.0; p.stages.len()],
            // the values of an observation a claim aggregates (`total(o)`, …)
            observed: (0..p.observes.len())
                .map(|k| {
                    let read = |e: &CExpr| matches!(e, CExpr::Agg(_, o) if *o == k);
                    p.claims.iter().any(|c| c.expr.any(&read)).then(Vec::new)
                })
                .collect(),
            live: 0,
            live_avg: TimeAverage::new(0.0, 0.0),
            admit_budget: None,
            next_dead: 0,
            next_lease: 0,
            regs: p.registers.iter().map(|r| r.init).collect(),
            preempted: false,
            preempted_now: 0.0,
            next_adm: 0,
            next_release: 0,
            arrivals: 0,
            ended: 0,
            turns: 0,
            events: 0,
            log: None,
            status_avg: ReqStatus::ALL.map(|_| TimeAverage::new(0.0, 0.0)).to_vec(),
            stays: ReqStatus::ALL.map(|_| LogHistogram::default()).to_vec(),
        }
    }

    /// Keep the log of the run (`engine::log`), at most `cap` events: what
    /// each session did, when, for a replay. The run is the same with or
    /// without it.
    pub fn logged(mut self, cap: usize) -> Self {
        self.log = Some(Log::new(self.p, cap));
        self
    }

    /// Log an event, if the run keeps a log.
    fn log(&mut self, kind: EventKind) {
        let now = self.now;
        if let Some(l) = &mut self.log {
            l.push(now, kind);
        }
    }

    /// Log an event about `sid`, if the run keeps a log: `kind` is built
    /// only then.
    fn log_who(&mut self, sid: usize, kind: impl FnOnce(Who) -> EventKind) {
        if self.log.is_none() {
            return;
        }
        let s = &self.sessions[sid];
        let who = Who {
            serial: s.serial,
            leg: s.leg.number(),
        };
        self.log(kind(who));
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
        let stuck = self.waiting_for_each_other();
        if stuck
            .iter()
            .any(|&s| matches!(self.sessions[s].status, Status::AwaitingSend(_)))
        {
            let who: Vec<String> = stuck.iter().map(|&s| self.waiting_at(s)).collect();
            return Err(format!(
                "the run ends with sessions that wait for each other: {}. A leg waits for \
                 memory that a send holds until its session's copy, and the session waits \
                 for the leg's send inside a hold another leg needs: a hold-and-wait cycle, \
                 which a finite `ttl` (the prefiller's lease expiry) breaks",
                who.join("; ")
            ));
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
                for a in &mut self.status_avg {
                    a.reset(now);
                }
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
            Ev::Start { stage, job } => self.on_start(stage, job),
            Ev::IterEnd { stage, epoch } => self.on_iter_end(stage, epoch),
            Ev::LeaseEnd { sid, serial, id } => {
                // a leg's lease has passed to its session when the leg ended
                let owner = std::iter::once(sid)
                    .chain(self.by_serial.get(&serial).copied())
                    .find_map(|s| {
                        let ss = &self.sessions[s];
                        (ss.serial == serial)
                            .then(|| ss.leases.iter().position(|l| l.id == id))
                            .flatten()
                            .map(|i| (s, i))
                    });
                // a send a copy reads ends with the read, not at its `ttl`
                if let Some((s, i)) = owner
                    && !self.sessions[s].leases[i].reading
                {
                    self.end_lease(s, i);
                    self.try_admit_all();
                }
            }
            Ev::PoolTick { pool } => self.pool_tick(pool),
            Ev::QueueTick { pool } => self.queue_tick(pool),
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
        self.run_ready();
        if self.error.is_some() {
            return;
        }
        self.end_of_settle();
    }

    /// Run the ready sessions, and admit what then fits, until none is
    /// ready: what an event leads to within its instant. A queue's tick
    /// runs each admission this far, not to the instant's end, where an
    /// iteration may start (`end_of_settle`).
    fn run_ready(&mut self) {
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
    }

    /// The checks, the iterations and the rescheduling once an event's
    /// sessions have settled.
    fn end_of_settle(&mut self) {
        // The memory invariant, `allocated + cached <= cap` in every
        // reachable configuration (`SerqLang.Step.invariant` proves it for
        // the pool relation; this checks the interpreter on every debug run:
        // `make check` runs the tests once more in a debug build). A pool
        // that acts at its ticks is over its cap between them by design,
        // outside the Lean fragment: then it has nothing cached.
        #[cfg(debug_assertions)]
        for (cp, pl) in self.p.pools.iter().zip(&self.pools) {
            let ticks = matches!(cp.preempt, Preempt::Leases { every: Some(_), .. });
            debug_assert!(
                pl.used + pl.cached <= pl.cap + 1e-6 || ticks && pl.cached <= 1e-6,
                "pool `{}`: used {} + cached {} > cap {} at t = {}",
                cp.name,
                pl.used,
                pl.cached,
                pl.cap,
                self.now
            );
        }
        // `reserve held`'s promise: what is allocated and what the live holds
        // reserved and have not allocated fit the cap, so a hold growing
        // within its reservation always finds room (#371).
        #[cfg(debug_assertions)]
        for (k, (cp, pl)) in self.p.pools.iter().zip(&self.pools).enumerate() {
            if cp.reserve_held {
                let out = self.outstanding(k, None);
                debug_assert!(
                    pl.used + out <= pl.cap + 1e-6,
                    "pool `{}`: used {} + reserved {} > cap {} at t = {}",
                    cp.name,
                    pl.used,
                    out,
                    pl.cap,
                    self.now
                );
            }
        }
        // Conservation (#277): a pool's `used` is what the sessions' holds
        // and leases have allocated on it, a prefix several of them read
        // counted once, and its `cached` the cached nodes of its trie. The
        // counters are kept apart from those records, so an allocation
        // released twice or never would otherwise pass unseen while the
        // pool stays within its cap.
        #[cfg(debug_assertions)]
        for (k, (cp, pl)) in self.p.pools.iter().zip(&self.pools).enumerate() {
            // (allocated less what was read, the units of the nodes read)
            let (private, read): (f64, f64) = self
                .sessions
                .iter()
                .flat_map(|s| {
                    let holds = s
                        .holds
                        .iter()
                        .flat_map(|h| &h.pools)
                        .filter(|e| e.pool == k)
                        .map(|e| (e.alloc - e.shared, &e.path));
                    // a lease set aside holds nothing (`suspend`)
                    let leases = s
                        .leases
                        .iter()
                        .filter(|l| l.pool == k && !l.suspended)
                        .map(|l| (l.alloc - l.shared, &l.path));
                    holds.chain(leases)
                })
                .fold((0.0, 0.0), |(a, r), (x, path)| {
                    (a + x, r + pl.cache.units(path))
                });
            let (cached, locked, per_hold) = pl.cache.totals();
            let held = private + locked;
            debug_assert!(
                (read - per_hold).abs() <= 1e-6 * read.max(1.0),
                "pool `{}`: the holds' paths have {read} units, the trie's locks {per_hold}, \
                 at t = {}",
                cp.name,
                self.now
            );
            // sums of fractional units drift with the pool's size, not with
            // what is held at this instant
            let scale = if pl.cap.is_finite() {
                pl.cap
            } else {
                pl.used.max(pl.cached).max(held)
            };
            let tol = 1e-9 * scale.max(1.0);
            debug_assert!(
                (pl.used - held).abs() <= tol && (pl.cached - cached).abs() <= tol,
                "pool `{}`: used {} against {held} held, cached {} against {cached} in entries, at t = {}",
                cp.name,
                pl.used,
                pl.cached,
                self.now
            );
            // one entry of `holders` per hold of the pool and per lease
            let entries: usize = self
                .sessions
                .iter()
                .map(|s| {
                    s.holds
                        .iter()
                        .flat_map(|h| &h.pools)
                        .filter(|e| e.pool == k)
                        .count()
                        + s.leases
                            .iter()
                            .filter(|l| l.pool == k && !l.suspended)
                            .count()
                })
                .sum();
            debug_assert_eq!(
                pl.holders.len(),
                entries,
                "pool `{}`: {} holders against {entries} holds and leases, at t = {}",
                cp.name,
                pl.holders.len(),
                self.now
            );
        }
        // An iteration starts only once every event of this instant has
        // been handled (a scheduler step sees all the arrivals up to it).
        let pending_now = self.heap.peek().is_some_and(|e| e.time <= self.now);
        for s in 0..self.stages.len() {
            if let Kind::Step {
                iter: None,
                residents,
                ..
            } = &self.stages[s].kind
                && (!residents.is_empty() || self.bound_waiting(s))
                && !pending_now
            {
                self.start_iteration(s);
                if self.error.is_some() {
                    return;
                }
            }
        }
        // after every iteration has started: an admission's request may
        // start a job on a `ps` stage declared before its engine (an L2
        // copy, #522), which a single pass in stage order had gone past
        for s in 0..self.stages.len() {
            if let Kind::Ps { dirty: true, .. } = &self.stages[s].kind {
                self.ps_reschedule(s);
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
                Kind::Shared { cap, .. } => {
                    // summed in job order: a HashMap's order is not the
                    // seed's, and a float sum depends on it
                    let mut ids: Vec<u64> = st.jobs.keys().copied().collect();
                    ids.sort_unstable();
                    ids.iter()
                        .map(|id| flows.get(id).map_or(0.0, |f| f.rate))
                        .sum::<f64>()
                        / cap
                }
                // a job in its run's wait is present, not served
                _ => {
                    if st.jobs.values().any(|j| j.started.is_some()) {
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
            pl.holders_avg.set(now, sessions_in(&pl.holders) as f64);
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
            cached_chain: HashMap::new(),
            stuck: false,
            leases: vec![],
            last_token: None,
            client: None,
            rng_wl: substream(self.p.seed, serial, 0, STREAM_WORKLOAD),
            rng: substream(self.p.seed, serial, 0, STREAM_SESSION),
            turn_count: 0,
            readies: (f64::NAN, 0),
            leg: Leg::No,
            legs: 0,
            forks: 0,
            req: None,
            calls: 0,
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
        self.check_given(sid);
        self.ready.push_back(sid);
        self.log_who(sid, |who| EventKind::Arrive { who });
    }

    /// Read every claim's `given` for a session whose `init` has run: one
    /// that fails it puts the claim out of scope for the run.
    fn check_given(&mut self, sid: usize) {
        let p = self.p;
        for (k, c) in p.claims.iter().enumerate() {
            if let Some(g) = &c.given
                && self.claims[k].out_of_scope.is_none()
                && self.eval(g, &Ctx::session(sid), Which::Session) == 0.0
            {
                self.claims[k].out_of_scope = Some(self.sessions[sid].serial);
            }
        }
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
        if let Some(all) = &mut self.observed[k] {
            all.push(v);
        }
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
        let turn = self.sessions[sid].attrs[p.slot_turn] as u32;
        self.log_who(sid, |who| EventKind::Turn { who, turn });
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
        // the server's statements follow the `Turn` in its block
        let fr = self.sessions[sid]
            .frames
            .last()
            .cloned()
            .expect("a turn runs in a frame");
        // the request is the run of server statements after the turn, as
        // the deployment view reads it (`view::deployment::request`)
        let sides = &p.sides[fr.block];
        let end = (fr.pc..sides.len())
            .find(|&i| sides[i] == Side::Workload)
            .unwrap_or(sides.len());
        self.sessions[sid].client = Some(ClientTurn {
            turn: self.sessions[sid].attrs[p.slot_turn],
            start: self.now,
            prefill_end: None,
            first: None,
            after: 0,
            depth: self.sessions[sid].frames.len(),
            block: fr.block,
            end,
        });
    }

    /// If the session's frame has passed the turn's server statements, the
    /// turn is answered: the client records `tpot` and `response`.
    fn answer_if_done(&mut self, sid: usize) {
        let s = &self.sessions[sid];
        let Some(c) = s.client else { return };
        if s.frames.len() != c.depth {
            return;
        }
        let fr = &s.frames[c.depth - 1];
        if fr.block != c.block || fr.pc < c.end {
            return;
        }
        self.sessions[sid].client = None;
        // a turn whose only output token a prefill sampled (`max_tokens =
        // 1`): its first token is that prefill's end (vLLM records its TTFT,
        // output_processor.py:692-702)
        if c.first.is_none()
            && let Some(at) = c.prefill_end
        {
            self.client_sample(sid, TTFT, at - c.start);
        }
        // as vLLM's benchmark client: the answer less the first token, over
        // the tokens after it (benchmarks/serve.py:625-628)
        if let Some(first) = c.first
            && c.after > 0
        {
            self.client_sample(sid, TPOT, (self.now - first) / c.after as f64);
        }
        self.client_sample(sid, RESPONSE, self.now - c.start);
    }

    /// A sample of the client's metric `k`, recorded now.
    fn client_sample(&mut self, sid: usize, k: usize, v: f64) {
        if self.warm {
            let s = &self.sessions[sid];
            let rec = (self.now, s.serial, s.attrs[self.p.slot_turn] as u32);
            self.client[k].w.push(v);
            self.client[k].samples.push(v);
            self.client[k].records.push(rec);
        }
    }

    /// The turn `sid` works for, in its session's slot: a leg's only while
    /// the session is still on the turn the leg was forked in.
    fn client_turn(&mut self, sid: usize) -> Option<(usize, &mut ClientTurn)> {
        let r = match self.sessions[sid].leg {
            Leg::No => sid,
            Leg::Of(r, _) => r,
            Leg::Orphan(_) => return None,
        };
        let turn = self.sessions[sid].attrs[self.p.slot_turn];
        let c = self.sessions[r].client.as_mut()?;
        (c.turn == turn).then_some((r, c))
    }

    /// The turn's first token is `at`, which the client sees now.
    fn first_token(&mut self, sid: usize, at: f64) {
        let Some((r, c)) = self.client_turn(sid) else {
            return;
        };
        c.first = Some(at);
        let start = c.start;
        self.client_sample(r, TTFT, at - start);
    }

    /// A decode-mode run of `sid`'s turn starts. If a prefill of the turn
    /// ended now, the token it sampled is the client's first, known now.
    fn decode_starts(&mut self, sid: usize) {
        let now = self.now;
        let Some((_, c)) = self.client_turn(sid) else {
            return;
        };
        if c.first.is_none() && c.prefill_end == Some(now) {
            self.first_token(sid, now);
        }
    }

    /// A token of `sid`'s turn committed now: a decode's, or a prefill's
    /// at its end. Before the first token, a decode's is the first, or
    /// makes the prefill's before it the first; after it, it counts.
    fn client_token(&mut self, sid: usize, decode: bool) {
        let now = self.now;
        let Some((_, c)) = self.client_turn(sid) else {
            return;
        };
        match (c.first, decode) {
            (Some(_), _) => c.after += 1,
            (None, false) => c.prefill_end = Some(now),
            (None, true) => {
                let first = c.prefill_end.unwrap_or(now);
                // the decode's own token is after the prefill's
                if c.prefill_end.is_some() {
                    c.after += 1;
                }
                self.first_token(sid, first);
            }
        }
    }

    fn end_session(&mut self, sid: usize) {
        if self.sessions[sid].status == Status::Ended {
            return;
        }
        if self.sessions[sid].leg != Leg::No {
            self.end_leg(sid);
            return;
        }
        if self.sessions[sid].legs > 0 {
            self.error = Some(format!(
                "session {} ends at t = {} while a leg it forked runs: a session waits for \
                 a leg by reading its send, and nothing read this one",
                self.sessions[sid].serial, self.now
            ));
            return;
        }
        self.log_who(sid, |who| EventKind::End { who });
        self.abandon_request(sid);
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

    /// `fork`: a leg of the session, ready now, with a copy of its
    /// attributes and a stream of its own.
    fn fork(&mut self, sid: usize, body: BlockId) {
        let p = self.p;
        let s = &mut self.sessions[sid];
        let n = s.forks;
        s.forks += 1;
        s.legs += 1;
        let leg = Session {
            serial: s.serial,
            attrs: s.attrs.clone(),
            frames: vec![Frame {
                block: body,
                pc: 0,
                kind: FrameKind::Plain,
            }],
            status: Status::Ready,
            holds: vec![],
            pending: None,
            trace: None,
            script: None,
            adm_seq: u64::MAX,
            preempt_pos: HashMap::new(),
            cached_chain: HashMap::new(),
            stuck: false,
            leases: vec![],
            last_token: None,
            client: None,
            rng_wl: s.rng_wl.clone(),
            rng: substream(p.seed, s.serial, n, STREAM_LEG),
            turn_count: s.turn_count,
            readies: (f64::NAN, 0),
            leg: Leg::Of(sid, n as u32),
            legs: 0,
            forks: 0,
            req: None,
            calls: 0,
        };
        let lid = match self.free.pop() {
            Some(i) => {
                self.sessions[i] = leg;
                i
            }
            None => {
                self.sessions.push(leg);
                self.sessions.len() - 1
            }
        };
        self.ready.push_back(lid);
        self.log_who(lid, |who| EventKind::Fork { who });
    }

    /// A hold that can never fit refuses the request: the session ends,
    /// and so does the session of a refused leg. The legs of an ended
    /// session run on as orphans (vLLM's push proxy awaits the prefill leg
    /// whatever the decode leg's fate) and give back what they lease when
    /// they end (the prefiller frees blocks no registration will claim,
    /// nixl/push_scheduler.py:233-245).
    fn refuse(&mut self, sid: usize) {
        let session = match self.sessions[sid].leg {
            Leg::No => Some(sid),
            Leg::Of(parent, _) => {
                self.end_leg(sid);
                Some(parent)
            }
            Leg::Orphan(_) => {
                self.end_leg(sid);
                None
            }
        };
        if let Some(s) = session {
            for leg in self.sessions.iter_mut() {
                if let Leg::Of(p, n) = leg.leg
                    && p == s
                    && leg.status != Status::Ended
                {
                    leg.leg = Leg::Orphan(n);
                }
            }
            self.sessions[s].legs = 0;
            self.end_session(s);
        }
    }

    /// A leg reached the end of its block: what it leased and the sends it
    /// registered pass to its session, and a session whose load waits for a
    /// send goes on once it holds one, or once no leg is left to register
    /// one (it reads nothing there, as a read whose send expired). An
    /// orphan's leases end with it.
    fn end_leg(&mut self, sid: usize) {
        self.log_who(sid, |who| EventKind::End { who });
        self.abandon_request(sid);
        self.detach(sid);
        while let Some(h) = self.sessions[sid].holds.pop() {
            self.release_hold(sid, &h);
        }
        let Leg::Of(parent, _) = self.sessions[sid].leg else {
            while !self.sessions[sid].leases.is_empty() {
                self.end_lease(sid, 0);
            }
            self.sessions[sid].status = Status::Ended;
            self.sessions[sid].frames.clear();
            self.free.push(sid);
            return;
        };
        let mut leases = std::mem::take(&mut self.sessions[sid].leases);
        for l in &mut leases {
            l.attrs
                .get_or_insert_with(|| self.sessions[sid].attrs.clone());
            for h in self.pools[l.pool].holders.iter_mut().filter(|h| **h == sid) {
                *h = parent;
            }
        }
        self.sessions[parent].leases.extend(leases);
        self.sessions[sid].status = Status::Ended;
        self.sessions[sid].frames.clear();
        self.free.push(sid);
        self.sessions[parent].legs -= 1;
        // a load waiting for the send the leg left goes on (`send_ready`)
        if let Status::AwaitingSend(pl) = self.sessions[parent].status
            && self.send_ready(parent, pl)
        {
            self.sessions[parent].status = Status::Ready;
            self.ready.push_back(parent);
        }
    }

    /// The sessions and legs that wait for each other and nothing else: a
    /// hold that does not fit a pool every holder of which is one of them
    /// (a lease's or a send's holder is its session), and a load waiting
    /// for a send whose live legs all are. No event will release what they
    /// wait for.
    fn waiting_for_each_other(&self) -> Vec<usize> {
        let n = self.sessions.len();
        let live_legs = |s: usize| {
            self.sessions
                .iter()
                .enumerate()
                .filter(move |(_, x)| {
                    matches!(x.leg, Leg::Of(p, _) if p == s) && x.status != Status::Ended
                })
                .map(|(l, _)| l)
        };
        let mut stuck: Vec<bool> = self
            .sessions
            .iter()
            .map(|x| matches!(x.status, Status::Queued(_) | Status::AwaitingSend(_)))
            .collect();
        loop {
            let mut changed = false;
            for s in 0..n {
                if !stuck[s] {
                    continue;
                }
                let keep = match &self.sessions[s].status {
                    // no live leg will leave the send it waits for
                    Status::AwaitingSend(_) => live_legs(s).all(|l| stuck[l]),
                    Status::Queued(_) => {
                        let pending = self.sessions[s].pending.as_ref().expect("queued");
                        let short: Vec<usize> = pending
                            .pools
                            .iter()
                            // `need` as admission last read it, or the units
                            // as the hold asked for them
                            .filter(|w| {
                                !self.fits(w.pool, self.round_up(w.pool, w.need.max(w.units)))
                            })
                            .map(|w| w.pool)
                            .collect();
                        // a lease that expires frees its pool without an event
                        // of theirs
                        let expiring = |pl: usize| {
                            self.sessions.iter().any(|x| {
                                x.leases
                                    .iter()
                                    .any(|l| l.pool == pl && l.expires.is_finite())
                            })
                        };
                        !short.is_empty()
                            && short.iter().all(|&pl| {
                                let holders = &self.pools[pl].holders;
                                !holders.is_empty()
                                    && holders.iter().all(|&h| stuck[h])
                                    && !expiring(pl)
                            })
                    }
                    _ => false,
                };
                if !keep {
                    stuck[s] = false;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        (0..n).filter(|&s| stuck[s]).collect()
    }

    /// What a stuck session waits at, for an error.
    fn waiting_at(&self, s: usize) -> String {
        let x = &self.sessions[s];
        let who = if x.leg != Leg::No {
            format!("a leg of session {}", x.serial)
        } else {
            format!("session {}", x.serial)
        };
        match x.status {
            Status::Queued(pl) => format!("{who} waits at `{}`", self.p.pools[pl].name),
            _ => format!("{who} waits for the send its load reads"),
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
            Status::Ready | Status::AwaitingSend(_) | Status::Ended => {}
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
            if self.sessions[sid].req.is_some() && !self.request_next(sid) {
                return;
            }
            self.answer_if_done(sid);
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
                        for e in &h.pools {
                            self.sessions[sid].preempt_pos.remove(&e.pool);
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
                CStmt::Fork(b) => self.fork(sid, *b),
                CStmt::While(c, b) => {
                    let v = self.eval(c, &Ctx::session(sid), Which::Session);
                    if v != 0.0 && v != 1.0 {
                        self.error = Some(format!(
                            "`while ({})`: the guard is {v}, not 0 or 1",
                            self.p.show_expr(c)
                        ));
                        return;
                    }
                    if v == 1.0 {
                        // Retest the guard after this plain body frame returns.
                        self.sessions[sid].frames.last_mut().unwrap().pc -= 1;
                        self.sessions[sid].frames.push(Frame {
                            block: *b,
                            pc: 0,
                            kind: FrameKind::Plain,
                        });
                    }
                }
                CStmt::Loop(b) => self.sessions[sid].frames.push(Frame {
                    block: *b,
                    pc: 0,
                    kind: FrameKind::Loop,
                }),
                CStmt::Branch(pe, a, b) => {
                    // a guard is a test: 0 or 1. A draw is written
                    // `if (~bernoulli(p))`, a sample that is already 0 or 1,
                    // so the guard never sees a fraction on purpose
                    let pr = self.eval(pe, &Ctx::session(sid), Which::Session);
                    let take = if pr == 1.0 {
                        true
                    } else if pr == 0.0 {
                        false
                    } else {
                        self.error = Some(format!(
                            "`if ({})`: the guard is {pr}, not 0 or 1; a draw is written `if (~bernoulli(p))`",
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
                    prefix,
                    lease,
                } => {
                    let wanted = self.wanted(pools, sid);
                    let lease = lease
                        .as_ref()
                        .map(|(r, t)| (self.session_index(r, sid), Expiry::After(t)));
                    let chain = self.chain_of(prefix.as_deref(), sid);
                    let pending = Pending {
                        pools: wanted,
                        reuse: reuse.as_ref(),
                        cache: cache.as_ref(),
                        chain,
                        lease,
                        body: *body,
                        queued_at: self.now,
                        request: false,
                        part: false,
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
                CStmt::Shrink(r, e) => {
                    let pl = self.session_index(r, sid);
                    let units = self.amount(e, sid, &format!("shrink {}", self.p.pools[pl].name));
                    self.shrink(sid, pl, units);
                    self.try_admit_all();
                }
                CStmt::Drop(r) => {
                    let pl = self.session_index(r, sid);
                    let serial = self.sessions[sid].serial;
                    let gone = self.pools[pl].cache.drop_owner(serial);
                    self.pools[pl].cached -= gone;
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
                CStmt::Request {
                    stage,
                    input,
                    output,
                } => {
                    let st = self.session_index(stage, sid);
                    let name = &p.stages[st].name;
                    let input = self.amount(input, sid, &format!("{name} input"));
                    let output_expr = output;
                    let output = self.amount(output, sid, &format!("{name} output"));
                    // a request generates at least one token (vLLM's
                    // `max_tokens`, sampling_params.py:612-614)
                    if output < 1.0 {
                        if self.error.is_none() {
                            self.error = Some(format!(
                                "`{name} (…, {})`: the request generates {output}, and a request \
                                 generates at least one token",
                                self.p.show_expr(output_expr)
                            ));
                        }
                        return;
                    }
                    let CStageKind::Step(step) = &p.stages[st].kind else {
                        unreachable!("validation: a request is to an engine")
                    };
                    let spec = step.request.as_deref();
                    self.sessions[sid].calls += 1;
                    let call = self.sessions[sid].calls;
                    self.sessions[sid].req = Some(Req {
                        stage: st,
                        spec,
                        memory: step.memory,
                        input,
                        output,
                        known: input,
                        generated: 0.0,
                        hit: 0.0,
                        loaded: 0.0,
                        preempted: false,
                        depth: 0,
                        finished: false,
                        first: false,
                        phase: Phase::Admitted,
                        call,
                        status: None,
                        since: f64::NAN,
                        reads: None,
                        unread: false,
                    });
                    if spec.is_some_and(|r| !r.pools.is_empty()) {
                        self.request_ask(sid, false);
                        return;
                    }
                    // an engine that takes nothing admits the request as it
                    // is called
                    if let Some(r) = spec {
                        self.observe_for(sid, r.records.reused, 0.0);
                        self.observe_for(sid, r.records.prefill, input);
                        let attrs = &mut self.sessions[sid].attrs;
                        attrs[r.records.admitted] = self.now;
                        attrs[r.records.last_reused] = 0.0;
                        attrs[r.records.last_prefill] = input;
                    }
                    if !self.request_next(sid) {
                        return;
                    }
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
                    let st = self.session_index(stage, sid);
                    // the kernel's spelling: `run E decode (…)`
                    let m = match mode {
                        RunMode::Plain => "",
                        RunMode::Prefill => " prefill",
                        RunMode::Decode => " decode",
                    };
                    let what = format!("run {}{m}", self.p.stages[st].name);
                    let w = self.amount(work, sid, &what);
                    // a connector's copy: its transfer starts (and a read's
                    // source's send with it)
                    if let Some(r) = &self.sessions[sid].req
                        && matches!(r.phase, Phase::Loading | Phase::Saving)
                    {
                        self.copy_starts(sid);
                    }
                    let wait = latency
                        .as_ref()
                        .map_or(0.0, |l| self.amount(l, sid, &format!("{what} latency")));
                    if matches!(self.stages[st].kind, Kind::Shared { .. }) {
                        let mut stages = vec![st];
                        for r in also {
                            stages.push(self.session_index(r, sid));
                        }
                        self.start_flow(stages, Some(sid), w, wait);
                        return;
                    }
                    let g = growing.as_ref().map(|r| self.session_index(r, sid));
                    if *mode == RunMode::Decode {
                        self.decode_starts(sid);
                    }
                    self.start_job(st, Some(sid), w, *mode, g, wait);
                    return;
                }
            }
        }
    }

    /// Put the session's request in the queue of the first pool its engine
    /// takes; `front`: back from a preemption (vLLM's `prepend_request`).
    fn request_ask(&mut self, sid: usize, front: bool) {
        let spec = self.sessions[sid]
            .req
            .as_ref()
            .and_then(|r| r.spec)
            .expect("a request to an engine that takes");
        let pools = self.wanted(&spec.pools, sid);
        let chain = self.chain_of(spec.prefix.as_deref(), sid);
        // a KV relation's source keeps the request's memory past its end, a
        // send for the reader's load (`CRequest::send`)
        let r = self.sessions[sid].req.as_ref().expect("a request");
        let lease = match &self.p.stages[r.stage].kind {
            CStageKind::Step(st) => (st.request.as_ref())
                .and_then(|q| q.send.as_ref())
                .zip(r.memory)
                .map(|(sd, m)| (m, Expiry::Fixed(sd.ttl.unwrap_or(f64::INFINITY)))),
            _ => None,
        };
        let pending = Pending {
            pools,
            reuse: spec.reuse.as_ref(),
            cache: spec.cache.as_ref(),
            chain,
            lease,
            body: 0,
            queued_at: self.now,
            request: true,
            part: false,
        };
        self.enqueue_hold(sid, pending, front);
    }

    /// `request.hit`: what the request may read of the cache in the
    /// engine's memory pool, `min(cachedin(P), reuse)`, read when it is
    /// tried for admission.
    fn request_hit(&mut self, sid: usize) {
        let Some(r) = &self.sessions[sid].req else {
            return;
        };
        let (memory, reuse) = (r.memory, r.spec.and_then(|s| s.reuse.as_ref()));
        let reuse = reuse.map(|e| self.eval(e, &Ctx::session(sid), Which::Session).max(0.0));
        let cached = memory.map_or(0.0, |pl| self.own_entry_size(pl, sid));
        let hit = reuse.map_or(cached, |b| cached.min(b));
        self.sessions[sid].req.as_mut().expect("a request").hit = hit;
        // what its connector loads beyond it, which `taking` reads
        let st = self.sessions[sid].req.as_ref().expect("a request").stage;
        // a request that took its KV `from` no source has none to read
        let remote = self
            .connector(st)
            .and_then(|c| c.from)
            .is_none_or(|a| self.sessions[sid].attrs[a] >= 0.0);
        let mut unread = false;
        let loaded = match self
            .connector(st)
            .and_then(|c| c.load.as_ref())
            .filter(|_| remote)
        {
            // a read of a send gone (its `ttl`) with no leg left to leave
            // another reads nothing: the request computes the tokens
            Some(t)
                if t.source.as_ref().is_some_and(|src| {
                    let pl = self.session_index(src, sid);
                    self.send_of(sid, pl).is_none() && self.sessions[sid].legs == 0
                }) =>
            {
                unread = true;
                0.0
            }
            Some(t) => {
                let what = format!("connector of {} load", self.p.stages[st].name);
                self.amount(&t.amount, sid, &what)
            }
            None => 0.0,
        };
        let r = self.sessions[sid].req.as_mut().expect("a request");
        r.loaded = loaded;
        r.unread = unread;
    }

    /// The request's load has ended: when, and a request that has KV for
    /// every token it has computes its last again, to sample the next
    /// (scheduler.py:3075-3076).
    fn load_ended(&mut self, sid: usize) {
        let r = self.sessions[sid].req.as_ref().expect("a request");
        let (st, known) = (r.stage, r.known);
        if let Some(c) = self.connector(st) {
            self.sessions[sid].attrs[c.load_done] = self.now;
        }
        if let Some((hi, k)) = self.request_kv(sid) {
            let e = &mut self.sessions[sid].holds[hi].pools[k];
            if e.pos >= known && known >= 1.0 {
                e.pos = known - 1.0;
            }
        }
    }

    /// After its load, the request's admission is read again, with nothing
    /// left to load: the pools it takes now that it had taken none of, it
    /// waits for, as a part of its hold (a running slot: vLLM's request
    /// whose remote KV has arrived waits again, scheduler.py:1264-1267).
    /// Whether it waits.
    fn request_more(&mut self, sid: usize) -> bool {
        let r = self.sessions[sid].req.as_mut().expect("a request");
        r.loaded = 0.0;
        r.phase = Phase::Waiting;
        let Some(spec) = r.spec else {
            return false;
        };
        let Some(hi) = self.sessions[sid].holds.iter().rposition(|h| h.request) else {
            return false;
        };
        let wanted = self.wanted(&spec.pools, sid);
        let held: Vec<(usize, f64)> = self.sessions[sid].holds[hi]
            .pools
            .iter()
            .map(|e| (e.pool, e.alloc))
            .collect();
        let more: Vec<Wanted<'p>> = wanted
            .into_iter()
            .filter(|w| {
                w.units > 0.0 && held.iter().all(|&(pl, alloc)| pl != w.pool || alloc <= 0.0)
            })
            .collect();
        if more.is_empty() {
            return false;
        }
        let pending = Pending {
            pools: more,
            reuse: None,
            cache: None,
            chain: Vec::new().into(),
            lease: None,
            body: 0,
            queued_at: self.now,
            request: false,
            part: true,
        };
        self.enqueue_hold(sid, pending, false);
        true
    }

    /// Give back the request's parts (`Hold::part`), which go with it.
    fn end_parts(&mut self, sid: usize) {
        while let Some(i) = self.sessions[sid].holds.iter().rposition(|h| h.part) {
            let h = self.sessions[sid].holds.remove(i);
            for e in &h.pools {
                let read = (&h.chain[..], e.shared, &e.path[..]);
                self.release_units(sid, e.pool, e.alloc, e.pos, None, read);
            }
        }
    }

    /// The kernel block of engine `st`'s connector's load (`save` false) or
    /// save (`Program::kernel`).
    fn transfer_block(&self, st: usize, save: bool) -> Option<BlockId> {
        let t = self.p.transfer_blocks.as_ref()?;
        if save { t.save[st] } else { t.load[st] }
    }

    /// Engine `st`'s connector, if it has one.
    fn connector(&self, st: usize) -> Option<&'p CConnector> {
        match &self.p.stages[st].kind {
            CStageKind::Step(step) => step.connector.as_deref(),
            _ => None,
        }
    }

    /// Run block `b` of engine `st`'s connector for `sid`'s request, its
    /// attribute `slot` set to `v`, in phase `phase`.
    fn connector_block(&mut self, sid: usize, slot: usize, v: f64, b: BlockId, phase: Phase) {
        let kind = if phase == Phase::Saving {
            Transfer::Save
        } else {
            Transfer::Load
        };
        // a load waiting for its source's send was registered then
        let waited = self.sessions[sid]
            .req
            .as_ref()
            .is_some_and(|r| matches!(r.status, Some((ReqStatus::WaitingForKv, _))));
        if !waited {
            self.log_transfer(sid, kind, Moved::Registered);
        }
        let s = &mut self.sessions[sid];
        s.attrs[slot] = v;
        let depth = s.frames.len();
        let r = s.req.as_mut().expect("a request");
        r.depth = depth;
        r.phase = phase;
        s.frames.push(Frame {
            block: b,
            pc: 0,
            kind: FrameKind::Plain,
        });
        self.restatus(sid);
    }

    /// The request's hold and the place of the engine's memory pool in it.
    fn request_kv(&self, sid: usize) -> Option<(usize, usize)> {
        let s = &self.sessions[sid];
        let memory = s.req.as_ref()?.memory?;
        let hi = s.holds.iter().rposition(|h| h.request)?;
        let k = s.holds[hi].pools.iter().position(|e| e.pool == memory)?;
        Some((hi, k))
    }

    /// The request's KV position: what of its tokens has KV.
    fn request_pos(&self, sid: usize) -> f64 {
        self.request_kv(sid)
            .map_or(0.0, |(hi, k)| self.sessions[sid].holds[hi].pools[k].pos)
    }

    /// The request is admitted: what the engine records of the admission.
    fn request_admitted(&mut self, sid: usize) {
        let pos = self.request_pos(sid);
        let now = self.now;
        let r = self.sessions[sid].req.as_mut().expect("a request");
        r.phase = Phase::Admitted;
        let (known, spec, loaded, st) = (r.known, r.spec, r.loaded, r.stage);
        if let Some(spec) = spec {
            // what it computes: past what it read and loads, and its last
            // token again when a load brings every one (`load_ended`)
            let prefill = if loaded > 0.0 && known >= 1.0 {
                known - (pos + loaded).min(known - 1.0)
            } else {
                known - pos - loaded
            };
            self.observe_for(sid, spec.records.reused, pos);
            self.observe_for(sid, spec.records.prefill, prefill);
            let attrs = &mut self.sessions[sid].attrs;
            attrs[spec.records.admitted] = now;
            attrs[spec.records.last_reused] = pos;
            attrs[spec.records.last_prefill] = prefill;
        }
        if let Some(c) = self.connector(st) {
            self.observe_for(sid, c.record, loaded);
            self.sessions[sid].attrs[c.loaded] = loaded;
            // a read of a send gone is over as it is admitted
            if self.sessions[sid].req.as_ref().is_some_and(|r| r.unread) {
                self.sessions[sid].attrs[c.load_done] = now;
            }
        }
    }

    /// Move the session's request on, its session ready again: start its
    /// prefill once admitted; at the prefill's end sample a token and
    /// decode; at the decode's end give its pools back. Returns whether the
    /// request has ended, and the session goes on with its statements.
    fn request_next(&mut self, sid: usize) -> bool {
        let r = self.sessions[sid].req.clone().expect("a request");
        // the memory pool grows with the KV position, if the request holds it
        let grow = self
            .request_kv(sid)
            .map(|_| r.memory.expect("a memory pool"));
        // a connector's block runs as the session's statements until it is done
        if matches!(r.phase, Phase::Loading | Phase::Saving)
            && self.sessions[sid].frames.len() > r.depth
        {
            return true;
        }
        // its load done: when, the last token computed again, and the pools
        // its admission takes now
        if r.phase == Phase::Loading {
            // a read ends the source's send it read (vLLM frees the source's
            // blocks at `finished_sending`, scheduler.py:3135-3138)
            if let Some(src) = self
                .connector(r.stage)
                .and_then(|c| c.load.as_ref())
                .and_then(|t| t.source.as_ref())
            {
                let pl = self.session_index(src, sid);
                self.end_send(sid, pl);
                self.try_admit_all();
            }
            self.log_transfer(sid, Transfer::Load, Moved::Done);
            self.load_ended(sid);
            if self.request_more(sid) {
                return false;
            }
        }
        let r = self.sessions[sid].req.clone().expect("a request");
        // its connector loads first, into the hold it was admitted with
        let load = self
            .connector(r.stage)
            .and_then(|c| c.load.as_ref().map(|t| (c.loaded, t.source.as_ref())))
            .zip(self.transfer_block(r.stage, false));
        if r.phase == Phase::Admitted
            && r.loaded > 0.0
            && let Some(((slot, source), b)) = load
        {
            // a read waits for its source's send: a push's leg's prefill
            if let Some(src) = source {
                let pl = self.session_index(src, sid);
                if !self.send_ready(sid, pl) {
                    self.log_transfer(sid, Transfer::Load, Moved::Registered);
                    self.sessions[sid].status = Status::AwaitingSend(pl);
                    self.restatus(sid);
                    return false;
                }
                // no send to read (expired, or no leg left to leave one):
                // the source's blocks are gone, and the request computes
                // those tokens itself, waiting first for the slot its
                // admission left for after the load (vLLM: a read of
                // invalid blocks is recomputed, scheduler.py:3140, 3254-3257)
                if self.send_of(sid, pl).is_none() {
                    self.load_nothing(sid);
                    if self.request_more(sid) {
                        return false;
                    }
                    let r = self.sessions[sid].req.clone().expect("a request");
                    return self.request_prefill(sid, &r, grow);
                }
            }
            self.connector_block(sid, slot, r.loaded, b, Phase::Loading);
            return true;
        }
        match r.phase {
            Phase::Admitted | Phase::Loading | Phase::Waiting if r.known > 0.0 => {
                return self.request_prefill(sid, &r, grow);
            }
            // nothing to prefill: it decodes from its first iteration
            Phase::Admitted | Phase::Loading | Phase::Waiting => {}
            Phase::Prefill => {
                let now = self.now;
                let req = self.sessions[sid].req.as_mut().unwrap();
                req.generated += 1.0;
                req.known += 1.0;
                if !req.first {
                    req.first = true;
                    if let Some(spec) = req.spec {
                        self.sessions[sid].attrs[spec.records.first] = now;
                    }
                }
            }
            Phase::Decode => {
                let req = self.sessions[sid].req.as_mut().unwrap();
                req.known += req.output - req.generated;
                req.generated = req.output;
                req.finished = true;
                // its connector saves before its pools go back
                // (`request_finished` keeps the blocks, lmcache_mp_connector.py:952)
                if let Some(c) = self.connector(r.stage)
                    && let Some(t) = &c.save
                    && let Some(b) = self.transfer_block(r.stage, true)
                {
                    let what = format!("connector of {} save", self.p.stages[r.stage].name);
                    let n = self.amount(&t.amount, sid, &what);
                    self.connector_block(sid, c.saved, n, b, Phase::Saving);
                    return true;
                }
                return self.request_end(sid);
            }
            Phase::Saving => {
                self.log_transfer(sid, Transfer::Save, Moved::Done);
                return self.request_end(sid);
            }
        }
        let r = self.sessions[sid].req.as_mut().unwrap();
        r.phase = Phase::Decode;
        let (stage, work) = (r.stage, (r.output - r.generated).max(0.0));
        // a request that sampled its one token at the prefill's end (a
        // prefiller's `max_tokens = 1`) decodes nothing: its token is not
        // the client's first unless a decode follows
        if work > 0.0 {
            self.decode_starts(sid);
        }
        self.start_job(stage, Some(sid), work, RunMode::Decode, grow, 0.0);
        self.restatus(sid);
        false
    }

    /// The request admitted to load `loaded` tokens finds no send to read:
    /// it loads nothing and computes them. What its admission recorded, the
    /// loaded tokens and the prefill (`E.loaded`, `E.prefill`, and the
    /// observations of each), becomes what it does: 0 loaded, its tokens
    /// past what it read from the cache prefilled.
    fn load_nothing(&mut self, sid: usize) {
        let r = self.sessions[sid].req.as_ref().expect("a request");
        let (st, spec, loaded, known) = (r.stage, r.spec, r.loaded, r.known);
        // the load registered when it began to wait is over, having read
        // nothing: when, as a load's end
        self.log_transfer(sid, Transfer::Load, Moved::Done);
        if let Some(c) = self.connector(st) {
            self.sessions[sid].attrs[c.loaded] = 0.0;
            self.sessions[sid].attrs[c.load_done] = self.now;
            self.amend(sid, c.record, loaded, 0.0);
        }
        if let Some(spec) = spec {
            let attrs = &self.sessions[sid].attrs;
            let (was, read) = (
                attrs[spec.records.last_prefill],
                attrs[spec.records.last_reused],
            );
            let now = known - read;
            self.sessions[sid].attrs[spec.records.last_prefill] = now;
            self.amend(sid, spec.records.prefill, was, now);
        }
    }

    /// Replace the latest sample `old` of observation `k` that `sid`'s
    /// session recorded in its turn by `new`: a value its request recorded
    /// at admission that the run then did otherwise (`load_nothing`). The
    /// running statistics are rebuilt from the samples, in their order.
    fn amend(&mut self, sid: usize, k: usize, old: f64, new: f64) {
        if let Some(all) = &mut self.observed[k]
            && let Some(x) = all.iter_mut().rev().find(|x| **x == old)
        {
            *x = new;
        }
        let s = &self.sessions[sid];
        let (serial, turn) = (s.serial, s.attrs[self.p.slot_turn] as u32);
        let o = &mut self.observes[k];
        let at = o
            .records
            .iter()
            .zip(&o.samples)
            .rposition(|(r, x)| r.1 == serial && r.2 == turn && *x == old);
        if let Some(i) = at {
            o.samples[i] = new;
            o.w = Welford::new();
            for &x in &o.samples {
                o.w.push(x);
            }
        }
    }

    /// Start the request's prefill: what of its tokens has no KV.
    fn request_prefill(&mut self, sid: usize, r: &Req<'p>, grow: Option<usize>) -> bool {
        let work = r.known - self.request_pos(sid);
        self.sessions[sid].req.as_mut().unwrap().phase = Phase::Prefill;
        self.start_job(r.stage, Some(sid), work, RunMode::Prefill, grow, 0.0);
        self.restatus(sid);
        false
    }

    /// The send of pool `pl` (a source's memory) a read of `sid`'s would
    /// read: the latest it holds, its source's call the last before.
    fn send_of(&self, sid: usize, pl: usize) -> Option<usize> {
        self.sessions[sid]
            .leases
            .iter()
            .enumerate()
            .filter(|(_, l)| l.pool == pl && !l.suspended && l.send.is_some())
            .max_by_key(|(_, l)| l.id)
            .map(|(i, _)| i)
    }

    /// Whether a read of `pl` by `sid` need wait no more: it holds a send of
    /// it, or no leg is left to leave one (it then reads nothing).
    fn send_ready(&self, sid: usize, pl: usize) -> bool {
        self.send_of(sid, pl).is_some() || self.sessions[sid].legs == 0
    }

    /// The request has generated its output: it gives its pools back,
    /// caching per `cache` with `request.finished` 1, and its session goes on.
    fn request_end(&mut self, sid: usize) -> bool {
        self.sessions[sid].req.as_mut().expect("a request").finished = true;
        if let Some(hi) = self.sessions[sid].holds.iter().rposition(|h| h.request) {
            self.end_parts(sid);
            let h = self.sessions[sid].holds.remove(hi);
            let leases = self.sessions[sid].leases.len();
            self.end_hold(sid, &h);
            // its blocks kept for a reader: finished, with a send
            let sent = (self.sessions[sid].leases.len() > leases).then_some(leases);
            self.request_finished(sid, sent);
            for e in &h.pools {
                self.sessions[sid].preempt_pos.remove(&e.pool);
            }
            let slot_computed = self.p.slot_computed;
            self.sessions[sid].attrs[slot_computed] = 0.0;
            self.sessions[sid].req = None;
            self.try_admit_all();
        } else {
            self.request_finished(sid, None);
            self.sessions[sid].req = None;
        }
        true
    }

    /// `sid`'s request has ended: `finished`, kept so while the send at
    /// `sent` among its session's leases holds its blocks; its stay in the
    /// status it was in ends (a save's `finished` goes on in its send).
    fn request_finished(&mut self, sid: usize, sent: Option<usize>) {
        let Some(r) = self.sessions[sid].req.as_mut() else {
            return;
        };
        let (old, since) = (r.status.take(), r.since);
        let now = self.now;
        let was_finished = matches!(old, Some((ReqStatus::Finished, _)));
        match (sent, old) {
            (Some(i), _) => {
                if !was_finished {
                    if let Some((st, _)) = old {
                        self.leave_status(st, since);
                    }
                    self.status_avg[ReqStatus::Finished.index()].add(now, 1.0);
                }
                let s = self.sessions[sid].leases[i].send.as_mut().expect("a send");
                s.since = if was_finished { since } else { now };
            }
            (None, Some((st, _))) => self.leave_status(st, since),
            (None, None) => {}
        }
        if !was_finished {
            self.log_status(sid, ReqStatus::Finished, RunMode::Plain);
        }
        if sent.is_some() {
            self.log_transfer(sid, Transfer::Send, Moved::Registered);
        }
    }

    /// A reader's load has read the source's send in pool `pl`: the one its
    /// copy chose when it started (`Req::reads`), which ends; gone (its
    /// `ttl`), none. A copy that never started (no copy run) chose none: the
    /// latest the session holds, or one a tick set aside.
    fn end_send(&mut self, sid: usize, pl: usize) {
        let reads = self.sessions[sid].req.as_mut().and_then(|r| r.reads.take());
        let i = match reads {
            Some(id) => self.sessions[sid].leases.iter().position(|l| l.id == id),
            None => self.send_of(sid, pl),
        };
        if let Some(i) = i {
            self.end_lease(sid, i);
        }
    }

    /// The request's status as its session's state says (`ReqStatus`): `None`
    /// while the session is ready (no time passes) or holds no request.
    fn req_status(&self, sid: usize) -> Option<(ReqStatus, RunMode)> {
        let s = &self.sessions[sid];
        let r = s.req.as_ref()?;
        let plain = |x| Some((x, RunMode::Plain));
        match r.phase {
            // whatever its connector's transfer waits for
            Phase::Loading => return plain(ReqStatus::WaitingForKv),
            Phase::Saving => return plain(ReqStatus::Finished),
            _ => {}
        }
        let mode = |st: usize, id: u64| self.stages[st].jobs.get(&id).map(|j| j.mode);
        match s.status {
            Status::AwaitingSend(_) => plain(ReqStatus::WaitingForKv),
            // vLLM's request is `PREEMPTED` until it runs, through its load
            // (scheduler.py:3088-3091)
            Status::Queued(_) if r.preempted => plain(ReqStatus::Preempted),
            Status::Queued(_) => plain(ReqStatus::Waiting),
            // a resident stalled for room to grow is running all the same
            Status::InStage(st, id) | Status::Growing(_, _, Some((st, id))) if st == r.stage => {
                Some((ReqStatus::Running, mode(st, id)?))
            }
            _ => None,
        }
    }

    /// Read `sid`'s request's status again (`req_status`): on a change its
    /// stay in the old one ends and the log says so; a change of mode alone
    /// is no new stay.
    fn restatus(&mut self, sid: usize) {
        let Some(new) = self.req_status(sid) else {
            return;
        };
        let r = self.sessions[sid].req.as_ref().expect("a request");
        let (old, since) = (r.status, r.since);
        if old == Some(new) {
            return;
        }
        let now = self.now;
        if old.map(|o| o.0) != Some(new.0) {
            if let Some((st, _)) = old {
                self.leave_status(st, since);
            }
            self.status_avg[new.0.index()].add(now, 1.0);
            self.sessions[sid].req.as_mut().unwrap().since = now;
        }
        self.sessions[sid].req.as_mut().unwrap().status = Some(new);
        self.log_status(sid, new.0, new.1);
    }

    /// A stay in `st` since `since` ends now.
    fn leave_status(&mut self, st: ReqStatus, since: f64) {
        let now = self.now;
        self.status_avg[st.index()].add(now, -1.0);
        if self.warm {
            self.stays[st.index()].push(now - since);
        }
    }

    /// `sid` ends with its request unfinished (refused): its stay ends.
    fn abandon_request(&mut self, sid: usize) {
        if self.sessions[sid].req.is_some() {
            self.request_finished(sid, None);
        }
    }

    /// The copy of `sid`'s request's transfer starts: the log's `start` of
    /// it, and of the source's send a read reads.
    fn copy_starts(&mut self, sid: usize) {
        let r = self.sessions[sid].req.as_ref().expect("a request");
        let (phase, st) = (r.phase, r.stage);
        if phase == Phase::Saving {
            self.log_transfer(sid, Transfer::Save, Moved::Start);
            return;
        }
        self.log_transfer(sid, Transfer::Load, Moved::Start);
        let Some(src) = self
            .connector(st)
            .and_then(|c| c.load.as_ref())
            .and_then(|t| t.source.as_ref())
        else {
            return;
        };
        // the send it reads: the latest the session holds of that member,
        // its source's call the last before the copy; the load ends that one
        let pl = self.session_index(src, sid);
        let read = self.send_of(sid, pl).map(|i| {
            let l = &mut self.sessions[sid].leases[i];
            l.reading = true;
            (l.id, l.send)
        });
        self.sessions[sid].req.as_mut().expect("a request").reads = read.map(|r| r.0);
        if let Some((_, Some(sd))) = read {
            self.log(EventKind::Transfer {
                who: sd.who,
                call: sd.call,
                stage: sd.stage,
                kind: Transfer::Send,
                moved: Moved::Start,
            });
        }
    }

    /// The log's `who` of `sid`.
    fn who(&self, sid: usize) -> Who {
        let s = &self.sessions[sid];
        Who {
            serial: s.serial,
            leg: s.leg.number(),
        }
    }

    fn log_status(&mut self, sid: usize, status: ReqStatus, mode: RunMode) {
        if self.log.is_none() {
            return;
        }
        let r = self.sessions[sid].req.as_ref().expect("a request");
        let (call, stage) = (r.call, r.stage);
        let who = self.who(sid);
        self.log(EventKind::Status {
            who,
            call,
            stage,
            status,
            mode,
        });
    }

    fn log_transfer(&mut self, sid: usize, kind: Transfer, moved: Moved) {
        if self.log.is_none() {
            return;
        }
        let r = self.sessions[sid].req.as_ref().expect("a request");
        let (call, stage) = (r.call, r.stage);
        let who = self.who(sid);
        self.log(EventKind::Transfer {
            who,
            call,
            stage,
            kind,
            moved,
        });
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
        self.fits_for(pl, units, None)
    }

    /// Whether `units` more fit pool `pl`: next to what is allocated, and
    /// under `reserve held` next to what the live holds' reservations have
    /// not allocated yet, the one hold entry `own` (session, hold, pool
    /// entry: a hold growing into its own reservation) left out.
    fn fits_for(&self, pl: usize, units: f64, own: Option<(usize, usize, usize)>) -> bool {
        let held = if self.p.pools[pl].reserve_held {
            self.outstanding(pl, own)
        } else {
            0.0
        };
        self.pools[pl].used + held + units <= self.pools[pl].cap + EPS
    }

    /// What the live hold entries on `pl` reserved and have not allocated
    /// (`reserve held`), each counted once, but `own`. Read from the
    /// sessions' holds, not the pool's `holders`, which lists a session once
    /// per hold or lease (the review of #371: a nested hold was counted
    /// twice).
    fn outstanding(&self, pl: usize, own: Option<(usize, usize, usize)>) -> f64 {
        let mut sum = 0.0;
        for (sid, s) in self.sessions.iter().enumerate() {
            for (hi, h) in s.holds.iter().enumerate() {
                for (k, e) in h.pools.iter().enumerate() {
                    if e.pool == pl && own != Some((sid, hi, k)) {
                        sum += (e.reserved - e.alloc).max(0.0);
                    }
                }
            }
        }
        sum
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

    /// The pools a hold statement asks for, its units evaluated now.
    /// Two references whose indices name the same member are the one pool
    /// twice, which only the run can see (`kv[i], kv[j]` with i = j, #309).
    fn wanted(&mut self, pools: &'p [(CRef, CExpr, Option<CExpr>)], sid: usize) -> Vec<Wanted<'p>> {
        let mut out: Vec<Wanted<'p>> = vec![];
        for (k, (r, e, f)) in pools.iter().enumerate() {
            let pool = self.session_index(r, sid);
            let what = format!("hold {}", self.p.pools[pool].name);
            if let Some(j) = out.iter().position(|w| w.pool == pool)
                && self.error.is_none()
            {
                let member = match self.p.pools[pool].index {
                    Some(i) => format!("{}[{i}]", self.p.pools[pool].name),
                    None => self.p.pools[pool].name.clone(),
                };
                self.error = Some(format!(
                    "`{}` and `{}` name the same member, `{member}`: a hold takes each pool once",
                    self.p.show_pool_ref(&pools[j].0),
                    self.p.show_pool_ref(&pools[k].0)
                ));
            }
            let units = self.amount(e, sid, &what);
            out.push(Wanted {
                pool,
                units,
                expr: e,
                reserve: f.as_ref(),
                need: 0.0,
                shared: 0.0,
            });
        }
        out
    }

    /// Put a hold request in its pool's queue. `front`: a preempted
    /// session re-enters at the head (vLLM `waiting.prepend_request`).
    /// Returns false if the request can never fit (the session ends).
    fn enqueue_hold(&mut self, sid: usize, pending: Pending<'p>, front: bool) -> bool {
        for w in &pending.pools {
            let pl = w.pool;
            // What admission waits for is the units, or the reservation
            // above them. A part that reads the deployment's state (a pool
            // or stage query, the clock, `budget_left`) may ask for less
            // later, so it is not judged now; one still over the cap when
            // the run ends is named (`head_fits`, #364). A part of
            // attributes and numbers asks the same at every try: above the
            // cap, the request can never fit, and is rejected.
            let mut need: f64 = 0.0;
            if !crate::ir::moves(w.expr) {
                need = w.units;
            }
            if let Some(f) = w.reserve
                && !crate::ir::moves(f)
            {
                let what = format!("hold {} reserve", self.p.pools[pl].name);
                need = need.max(self.amount(f, sid, &what));
            }
            if self.round_up(pl, need) > self.pools[pl].cap {
                self.pools[pl].rejected += 1;
                self.log_who(sid, |who| EventKind::Refuse { who });
                self.refuse(sid);
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
        self.log_who(sid, |who| EventKind::Wait { who, pool: pl });
        self.restatus(sid);
        self.schedule_queue_tick(pl);
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
        self.next_waiter_among(pl, None)
    }

    /// `next_waiter` among the sessions of `among` only, when given.
    fn next_waiter_among(&mut self, pl: usize, among: Option<&[usize]>) -> Option<(usize, usize)> {
        let queue = &self.pools[pl].queue;
        let ok = |sid: usize| among.is_none_or(|a| a.contains(&sid));
        if let Some(entry) = queue.iter().find(|e| ok(e.sid))
            && (entry.resumed || self.p.pools[pl].queue.is_none())
        {
            let index = queue.iter().position(|e| e.sid == entry.sid).unwrap();
            return Some((index, entry.sid));
        }
        let keys = self.p.pools[pl].queue.as_ref()?;
        let candidates: Vec<_> = queue.iter().map(|entry| entry.sid).collect();
        let mut best: Option<(KeyOrd, usize, usize)> = None;
        for (index, sid) in candidates.into_iter().enumerate() {
            if !ok(sid) {
                continue;
            }
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
        if pending.request {
            self.request_hit(sid);
        }
        for w in &mut pending.pools {
            let what = format!("hold {}", self.p.pools[w.pool].name);
            let u = self.amount(w.expr, sid, &what);
            w.units = u;
            w.need = match w.reserve {
                Some(f) => self.amount(f, sid, &format!("{what} reserve")).max(u),
                None => u,
            };
        }
        if pending.cache.is_some() {
            let reuse = self.reuse_now(&pending, sid);
            let serial = self.sessions[sid].serial;
            for w in &mut pending.pools {
                let chain = self.pool_chain(w.pool, &pending.chain, serial);
                let alloc = self.round_up(w.pool, w.units);
                let (limit, _) = self.read_limit(w.pool, &chain, (reuse, alloc), serial);
                w.shared = self.pools[w.pool].cache.locked_along(&chain, limit, serial);
            }
        }
        pending
    }

    fn reuse_now(&mut self, pending: &Pending<'p>, sid: usize) -> Option<f64> {
        pending
            .reuse
            .as_ref()
            .map(|e| self.eval(e, &Ctx::session(sid), Which::Session).max(0.0))
    }

    /// What a hold with a `cache` clause reads of its chain's cached prefix
    /// on `q` when admitted: the whole blocks of it, at most `reuse`. On a
    /// chain of shared segments, also at most what it allocates there: what
    /// it reads is part of its allocation, and a prefix another hold reads
    /// cannot shrink to fit it. On the session's own chain it consumes its
    /// whole entry, as the Lean model's `admitPool` does (`Exec.lean`), and
    /// what its allocation does not hold of it is gone. Returns (that, the
    /// whole blocks cached).
    fn read_limit(
        &self,
        q: usize,
        chain: &[Seg],
        (reuse, alloc): (Option<f64>, f64),
        serial: u64,
    ) -> (f64, f64) {
        let full = self.round_down(q, self.pools[q].cache.matched(chain, serial));
        let alloc = if self.is_own_chain(chain, serial) {
            f64::INFINITY
        } else {
            alloc
        };
        let bound = reuse.map_or(alloc, |r| r.min(alloc));
        (full.min(self.round_down(q, bound)), full)
    }

    /// The session's own chain, `(serial: ∞)`: a hold without `prefix`.
    fn is_own_chain(&self, chain: &[Seg], serial: u64) -> bool {
        matches!(chain, [(ns, k, n)]
            if *ns == self.p.slot_serial as u64 && *k == serial as f64 && n.is_infinite())
    }

    /// The chain a hold reads on pool `q`: its own on a pool without blocks
    /// (a slot pool, whose `cache` holds a slot, not a sequence: shared, one
    /// slot cached along a chain would be read by every hold as allocated
    /// already, and the slots' cap would hold no more), else the hold's.
    fn pool_chain(&self, q: usize, chain: &Chain, serial: u64) -> Chain {
        if self.pools[q].block.is_none() && !self.is_own_chain(chain, serial) {
            own_chain(self.p, serial)
        } else {
            chain.clone()
        }
    }

    /// Admit the selected waiting hold while every pool of its hold
    /// has room; a selection that does not fit blocks the rest. A pool whose
    /// queue is served by a stage (`admit_via`) is admitted from there.
    fn try_admit(&mut self, pl: usize) {
        // an engine admits its queue; a pool under `queue … every`, its ticks
        if self.p.pools[pl].admit_via.is_some() || self.p.pools[pl].queue_every.is_some() {
            return;
        }
        while let Some((index, sid)) = self.next_waiter(pl) {
            let pending = self.pending_now(sid);
            if !self.fits_all(&pending) {
                break;
            }
            self.pools[pl].queue.remove(index);
            self.admit(sid, pending);
        }
    }

    /// Whether a hold, evaluated now, fits every pool it names. The guard
    /// counts only allocated units: cached prefixes never block an
    /// admission (they are evicted as needed).
    fn fits_all(&self, pending: &Pending<'p>) -> bool {
        pending
            .pools
            .iter()
            .all(|w| self.fits(w.pool, self.round_up(w.pool, w.need) - w.shared))
    }

    /// The pools whose cap the head of a queue, evaluated as the run ends,
    /// asks more than: `(the pool asked, the queue's pool, what it asks)`.
    /// A hold whose units or `reserve` read the deployment's state was not
    /// rejected when it joined (#364); one that still cannot fit is named
    /// here, whether or not an admission tried it.
    fn heads_over_cap(&mut self) -> Vec<(usize, usize, f64)> {
        let error = self.error.clone();
        let mut over = vec![];
        for q in 0..self.pools.len() {
            if self.pools[q].queue.is_empty() {
                continue;
            }
            let Some((_, sid)) = self.next_waiter(q) else {
                continue;
            };
            let pending = self.pending_now(sid);
            for w in &pending.pools {
                if self.round_up(w.pool, w.need) > self.pools[w.pool].cap {
                    over.push((w.pool, q, w.need));
                }
            }
        }
        // a read at the end is the report's, not the run's
        self.error = error;
        over
    }

    /// `allocated(p)`: what `sid` has allocated in `pl`, in its holds and its
    /// leases.
    fn allocated_by(&self, pl: usize, sid: usize) -> f64 {
        let leases: f64 = self.sessions[sid]
            .leases
            .iter()
            .filter(|l| l.pool == pl && !l.suspended)
            .map(|l| l.alloc)
            .sum();
        self.held_in(sid, pl) + leases
    }

    /// A hold's chain: its `prefix`, evaluated now, or the session's own.
    fn chain_of(&mut self, prefix: Option<&'p [(CExpr, CExpr)]>, sid: usize) -> Chain {
        let Some(segs) = prefix else {
            return own_chain(self.p, self.sessions[sid].serial);
        };
        let ctx = Ctx::session(sid);
        let mut chain = Vec::with_capacity(segs.len());
        for (j, (k, n)) in segs.iter().enumerate() {
            let key = self.eval(k, &ctx, Which::Session);
            let len = self.eval(n, &ctx, Which::Session);
            let endless = len.is_infinite() && j + 1 < segs.len();
            if (key.is_nan() || len.is_nan() || len < 0.0 || endless) && self.error.is_none() {
                self.error = Some(format!(
                    "a `prefix` segment is ({key}: {len}): a key is a number, a length is not \
                     negative, and only the last segment is endless"
                ));
            }
            // an empty segment holds nothing to share: `(tools: 0, …)`, a
            // session without tools, is the chain without it
            if len > 0.0 {
                chain.push((key_space(k), key, len));
            }
        }
        Rc::from(chain)
    }

    /// The chain `cachedin(pl)` reads for the session: that of its waiting
    /// hold if it asks for `pl`, else of its innermost hold of `pl`, else the
    /// one it last cached along in `pl`, else its own. A `prefix` keys the
    /// pools its hold holds, not the others (#595).
    fn session_chain(&self, pl: usize, sid: usize) -> Chain {
        let s = &self.sessions[sid];
        if let Some(p) = s
            .pending
            .as_ref()
            .filter(|p| p.pools.iter().any(|w| w.pool == pl))
        {
            return p.chain.clone();
        }
        if let Some((hi, _)) = s.innermost(pl) {
            return s.holds[hi].chain.clone();
        }
        match self.sessions[self.root(sid)].cached_chain.get(&pl) {
            Some(c) => c.clone(),
            None => own_chain(self.p, s.serial),
        }
    }

    /// The session a leg is of, else the session itself: the one that keeps
    /// `cached_chain`, since its legs share its entries.
    fn root(&self, mut sid: usize) -> usize {
        while let Leg::Of(parent, _) = self.sessions[sid].leg {
            sid = parent;
        }
        sid
    }

    /// The cached prefix of the session's chain in `pl`, in whole blocks.
    fn own_entry_size(&self, pl: usize, sid: usize) -> f64 {
        let serial = self.sessions[sid].serial;
        let chain = self.pool_chain(pl, &self.session_chain(pl, sid), serial);
        self.round_down(pl, self.pools[pl].cache.matched(&chain, serial))
    }

    fn admit(&mut self, sid: usize, pending: Pending<'p>) {
        let p = self.p;
        let serial = self.sessions[sid].serial;
        let mut cached_first = None;
        let mut held = vec![];
        let reuse = self.reuse_now(&pending, sid);
        for w in &pending.pools {
            let q = w.pool;
            let need = self.round_up(q, w.units);
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
            // The prefix it reads is locked: allocated while it lasts, and
            // counted once however many holds read it (vLLM's `ref_cnt`).
            let (own, moved, path) = if pending.cache.is_some() {
                let chain = self.pool_chain(q, &pending.chain, serial);
                let (limit, full) = self.read_limit(q, &chain, (reuse, need), serial);
                let (own, moved, path) = self.pools[q].cache.lock(&chain, limit, serial);
                if limit < full - EPS {
                    let tag = DEAD_ENTRY + self.next_dead;
                    self.next_dead += 1;
                    let snap = self.sessions[sid].attrs.clone();
                    self.pools[q].cache.cut(&chain, own, tag, serial, &snap);
                }
                (own, moved, path)
            } else {
                (0.0, 0.0, vec![])
            };
            // what another hold already reads is allocated already; the rest
            // of what this one reads moves from the cache to the allocation
            // with the units it takes
            let read_already = own - moved;
            self.pools[q].cached -= moved;
            // `cached`: the consumed prefix (the largest, if several pools
            // of the hold had one)
            cached_first = Some(cached_first.map_or(own, |c: f64| c.max(own)));
            self.make_room(q, need - read_already);
            self.pools[q].used += need - read_already;
            self.pools[q].holders.push(sid);
            self.pools[q].admissions += 1;
            held.push(Held {
                pool: q,
                alloc: need,
                pos: own,
                // what the admission tested, held against later ones under
                // `reserve held`
                reserved: self.round_up(q, w.need),
                shared: own,
                path,
            });
        }
        let pl = first_pool(&pending);
        self.log_who(sid, |who| EventKind::Admit { who, pool: pl });
        if self.warm {
            let w = self.now - pending.queued_at;
            self.pools[pl].wait.push(w);
        }
        let seq = self.next_adm;
        self.next_adm += 1;
        let s = &mut self.sessions[sid];
        s.adm_seq = seq;
        // a request's part reads no cache: `cached` stays its admission's
        if !pending.part {
            s.attrs[p.slot_cached] = cached_first.unwrap_or(0.0);
        }
        s.pending = None;
        s.status = Status::Ready;
        s.holds.push(Hold {
            pools: held,
            grown: false,
            cache: pending.cache,
            chain: pending.chain,
            lease: pending.lease,
            body: pending.body,
            request: pending.request,
            part: pending.part,
        });
        if !pending.request && !pending.part {
            s.frames.push(Frame {
                block: pending.body,
                pc: 0,
                kind: FrameKind::Hold,
            });
        }
        self.ready.push_back(sid);
        if pending.request {
            self.request_admitted(sid);
        }
    }

    /// Evict cached prefixes until `need` more units fit, after a guard
    /// that passed.
    fn make_room(&mut self, pl: usize, need: f64) {
        self.evict_for(pl, need);
        debug_assert!(
            self.pools[pl].used + need <= self.pools[pl].cap + EPS,
            "make_room called without a passing guard"
        );
    }

    /// Evict cached prefixes until `need` more units fit, or none is left:
    /// a growth a pool's tick lets past the cap evicts all it can.
    fn evict_for(&mut self, pl: usize, need: f64) {
        let over = |s: &Self| s.pools[pl].used + s.pools[pl].cached + need > s.pools[pl].cap + EPS;
        // one victim is a linear scan; the heap pays off only for several
        let short = self.pools[pl].used + self.pools[pl].cached + need - self.pools[pl].cap;
        let several = self.pools[pl].block.is_some_and(|b| short > b + EPS);
        if self.evict_static[pl] && several {
            // The keys of the leaves not evicted do not change within one
            // call: key every leaf once, then re-key only the one that
            // shrank, and key a parent that a leaf's going left a leaf. The
            // same victims, in the same order, as `evict_one`.
            let leaves: Vec<usize> = self.pools[pl].cache.leaves().collect();
            let mut heap: BinaryHeap<std::cmp::Reverse<(KeyOrd, usize)>> = leaves
                .into_iter()
                .map(|n| std::cmp::Reverse((KeyOrd(self.entry_key(pl, n)), n)))
                .collect();
            while over(self) {
                let Some(std::cmp::Reverse((_, node))) = heap.pop() else {
                    break;
                };
                let next = match self.evict_entry(pl, node) {
                    (true, _) => Some(node),
                    (false, parent) => parent,
                };
                if let Some(n) = next {
                    heap.push(std::cmp::Reverse((KeyOrd(self.entry_key(pl, n)), n)));
                }
            }
        } else {
            while over(self) {
                if !self.evict_one(pl) {
                    break;
                }
            }
        }
    }

    /// The session a cache node's eviction key and spill read: its writer,
    /// while it lives, unless the node is dead (no session's any more).
    fn node_writer(&self, pl: usize, node: usize) -> Option<usize> {
        let n = self.pools[pl].cache.node(node);
        match n.dead {
            Some(_) => None,
            None => self.by_serial.get(&n.owner).copied(),
        }
    }

    fn entry_key(&mut self, pl: usize, node: usize) -> Vec<f64> {
        let n = self.pools[pl].cache.node(node);
        if let CEvict::Lru = &self.p.pools[pl].evict {
            return vec![n.last, n.seq as f64];
        }
        let (size, last, seq, snap) = (n.len, n.last, n.seq, n.snap.clone());
        let live = self.node_writer(pl, node);
        let queued = live.is_some_and(|s| matches!(self.sessions[s].status, Status::Queued(_)));
        let ctx = Ctx {
            sid: live,
            snap: if live.is_some() { None } else { Some(snap) },
            size,
            age: self.now - last,
            last,
            queued: if queued { 1.0 } else { 0.0 },
            ..Default::default()
        };
        let p = self.p;
        match &p.pools[pl].evict {
            CEvict::Lru => vec![last, seq as f64],
            CEvict::By(keys) => {
                let mut v: Vec<f64> = keys
                    .iter()
                    .map(|k| self.eval(k, &ctx, Which::Evict))
                    .collect();
                v.push(seq as f64);
                v
            }
        }
    }

    /// Evict the cached leaf (or its tail block) with the smallest key.
    fn evict_one(&mut self, pl: usize) -> bool {
        let leaves: Vec<usize> = self.pools[pl].cache.leaves().collect();
        if leaves.is_empty() {
            return false;
        }
        let mut best: Option<(Vec<f64>, usize)> = None;
        for n in leaves {
            let k = self.entry_key(pl, n);
            let better = match &best {
                None => true,
                Some((bk, _)) => lex_less(&k, bk),
            };
            if better {
                best = Some((k, n));
            }
        }
        let (_, node) = best.unwrap();
        self.evict_entry(pl, node);
        true
    }

    /// Evict leaf `node`'s tail block (or all of it): (whether it is still
    /// cached, the parent its going left an evictable leaf).
    fn evict_entry(&mut self, pl: usize, node: usize) -> (bool, Option<usize>) {
        let block = self.pools[pl].block;
        let n = self.pools[pl].cache.node(node);
        let units = match block {
            Some(b) => b,
            None => n.len,
        };
        let (owner, dead, last, snap) = (n.owner, n.dead, n.last, n.snap.clone());
        let live = self.node_writer(pl, node);
        let (removed, parent, gone) = self.pools[pl].cache.evict_tail(node, units);
        if self.trace_evict {
            eprintln!("EVICT {:.4} {} {removed}", self.now, dead.unwrap_or(owner));
        }
        if gone {
            self.pools[pl].evicted_entries += 1;
        }
        self.pools[pl].cached -= removed;
        self.pools[pl].evicted_units += removed;
        self.spill(pl, (owner, dead, live), removed, last, snap);
        (!gone, parent)
    }

    /// `units` evicted from `pl` go to its spill tier, if its predicate
    /// says so, as the entry of their writer (or of the dead node they
    /// were): `(owner, dead, live)`.
    fn spill(
        &mut self,
        pl: usize,
        (owner, dead, live): (u64, Option<u64>, Option<usize>),
        units: f64,
        last: f64,
        snap: Vec<f64>,
    ) {
        let Some(sp) = self.p.pools[pl].spill.clone() else {
            return;
        };
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
        let added = self.pools[to].cache.spill_in(
            &own_chain(self.p, owner)[0],
            owner,
            dead,
            units,
            snap,
            now,
            rseq,
        );
        self.pools[to].cached += added;
        self.pools[pl].spills += 1;
        self.start_job(sp.via, None, work, RunMode::Plain, None, 0.0);
    }

    /// A preempted or ended hold gives everything back at once.
    fn release_hold(&mut self, sid: usize, h: &Hold<'p>) {
        for e in &h.pools {
            let read = (&h.chain[..], e.shared, &e.path[..]);
            self.release_units(sid, e.pool, e.alloc, e.computed(h.grown), h.cache, read);
        }
    }

    /// The scope's end: every pool is given back, except the leased one,
    /// whose allocation stays the session's (neither evictable nor a
    /// preemption victim) until its `release`, the expiry or the session's
    /// end.
    fn end_hold(&mut self, sid: usize, h: &Hold<'p>) {
        for e in &h.pools {
            let (q, alloc, computed) = (e.pool, e.alloc, e.computed(h.grown));
            match h.lease {
                Some((lp, t)) if lp == q => {
                    let t = match t {
                        Expiry::After(e) => {
                            self.eval(e, &Ctx::session(sid), Which::Session).max(0.0)
                        }
                        Expiry::Fixed(t) => t,
                    };
                    let keep = h
                        .cache
                        .filter(|_| h.request)
                        .map(|c| self.eval(c, &Ctx::session(sid), Which::Session).max(0.0));
                    let id = self.next_lease;
                    self.next_lease += 1;
                    let send = h.request.then(|| {
                        let r = self.sessions[sid].req.as_ref().expect("a request");
                        SendOf {
                            who: self.who(sid),
                            call: r.call,
                            stage: r.stage,
                            since: self.now,
                        }
                    });
                    self.sessions[sid].leases.push(Lease {
                        id,
                        pool: q,
                        alloc,
                        computed,
                        cache: h.cache,
                        chain: h.chain.clone(),
                        shared: e.shared,
                        path: e.path.clone(),
                        expires: self.now + t,
                        attrs: None,
                        keep,
                        suspended: false,
                        send,
                        reading: false,
                    });
                    self.schedule_tick(q);
                    if t.is_finite() {
                        let serial = self.sessions[sid].serial;
                        self.at(self.now + t, Ev::LeaseEnd { sid, serial, id });
                    }
                }
                _ => {
                    let read = (&h.chain[..], e.shared, &e.path[..]);
                    self.release_units(sid, q, alloc, computed, h.cache, read)
                }
            }
        }
    }

    /// A lease ends: the units go back, `cache` applies.
    fn end_lease(&mut self, sid: usize, i: usize) {
        let l = self.sessions[sid].leases.remove(i);
        // a send done: its request keeps its blocks no more
        if let Some(sd) = l.send {
            self.leave_status(ReqStatus::Finished, sd.since);
            self.log(EventKind::Transfer {
                who: sd.who,
                call: sd.call,
                stage: sd.stage,
                kind: Transfer::Send,
                moved: Moved::Done,
            });
        }
        // a lease set aside holds nothing any more
        if l.suspended {
            return;
        }
        // a leg's lease caches by the leg's attributes, not the session's
        let saved = l
            .attrs
            .map(|a| std::mem::replace(&mut self.sessions[sid].attrs, a));
        let read = (&l.chain[..], l.shared, &l.path[..]);
        match l.keep {
            Some(x) => {
                self.release_kept(sid, l.pool, l.alloc, l.computed, Some(Keep::Read(x)), read)
            }
            None => self.release_units(sid, l.pool, l.alloc, l.computed, l.cache, read),
        }
        if let Some(a) = saved {
            self.sessions[sid].attrs = a;
        }
    }

    /// Give `alloc` units of `q` back, keeping `min(cache, computed)` of them
    /// cached (rounded down to blocks) along the hold's chain. `read`: the
    /// chain, the prefix of it the hold read through, and the nodes it
    /// locked there, which it lets go.
    fn release_units(
        &mut self,
        sid: usize,
        q: usize,
        alloc: f64,
        computed: f64,
        cache: Option<&'p CExpr>,
        read: (&[Seg], f64, &[usize]),
    ) {
        self.release_kept(sid, q, alloc, computed, cache.map(Keep::Clause), read);
    }

    /// `release_units`, what `cache` keeps given as its clause or as read.
    fn release_kept(
        &mut self,
        sid: usize,
        q: usize,
        alloc: f64,
        computed: f64,
        keep: Option<Keep<'p>>,
        (chain, shared, path): (&[Seg], f64, &[usize]),
    ) {
        let serial = self.sessions[sid].serial;
        let chain = self.pool_chain(q, &Rc::from(chain), serial);
        // what no other hold reads any more is cached again, until the
        // clause below says what of it stays (a node the session's own write
        // displaced goes); what another hold still reads stays allocated
        let (back, gone) = self.pools[q].cache.unlock(path, serial);
        self.pools[q].used -= alloc - (shared - back - gone);
        // one entry per admission: a hold nested in another on the same pool
        // leaves the outer one's when it ends
        if let Some(i) = self.pools[q].holders.iter().position(|&s| s == sid) {
            self.pools[q].holders.remove(i);
        }
        let mut delta = 0.0;
        if let Some(k) = keep {
            let want = match k {
                Keep::Clause(c) => self.eval(c, &Ctx::session(sid), Which::Session).max(0.0),
                Keep::Read(x) => x,
            };
            let keep = self.round_down(q, want.min(computed));
            let snap = self.sessions[sid].attrs.clone();
            let rseq = self.next_release;
            if keep > 0.0 {
                self.next_release += 1;
                let root = self.root(sid);
                self.sessions[root].cached_chain.insert(q, chain.clone());
            }
            let now = self.now;
            delta = self.pools[q]
                .cache
                .write(&chain, keep, path, serial, &snap, now, rseq);
        }
        // one addition: on the session's own entry, `back + delta` is what
        // it keeps
        self.pools[q].cached += back + delta;
        // a pool over its cap between its ticks caches only within the cap
        if matches!(
            self.p.pools[q].preempt,
            Preempt::Leases { every: Some(_), .. }
        ) {
            self.evict_for(q, 0.0);
        }
    }

    /// `release P`: the innermost hold on `pl` gives its allocation there
    /// back now, caching per its clause, and no longer holds `pl`; its scope
    /// end then has nothing left there. With no hold on `pl`, the session's
    /// lease of it ends (the transfer took the KV). Neither: a no-op (a
    /// hold re-executed after a preemption reaches the statement again).
    fn release_early(&mut self, sid: usize, pl: usize) {
        let Some((hi, k)) = self.sessions[sid].innermost(pl) else {
            // the lease that holds units, else one a tick set aside
            if let Some(i) = self
                .lease_of(sid, pl)
                .or_else(|| self.set_aside_of(sid, pl))
            {
                self.end_lease(sid, i);
            }
            return;
        };
        let h = &mut self.sessions[sid].holds[hi];
        let e = h.pools.remove(k);
        let computed = e.computed(h.grown);
        let cache = h.cache;
        let chain = h.chain.clone();
        self.release_units(
            sid,
            pl,
            e.alloc,
            computed,
            cache,
            (&chain, e.shared, &e.path),
        );
        self.sessions[sid].preempt_pos.remove(&pl);
    }

    /// `load P (n)`: the KV of `n` tokens arrived from outside the engine;
    /// the innermost hold's position on `pl` advances by `n`, which its
    /// allocation must cover (`grow` first, or allocate at admission, as
    /// vLLM's decoder allocates the whole prompt before it reads).
    fn load(&mut self, sid: usize, pl: usize, n: f64) {
        let name = &self.p.pools[pl].name;
        let Some((hi, k)) = self.sessions[sid].innermost(pl) else {
            self.error = Some(format!("`load {name}` outside a hold of `{name}`"));
            return;
        };
        let h = &mut self.sessions[sid].holds[hi];
        let e = &mut h.pools[k];
        if e.pos + n > e.alloc + EPS {
            self.error = Some(format!(
                "`load {name} ({n})`: the hold has {} allocated and {} computed; \
                 a load must fit the allocation (grow first)",
                e.alloc, e.pos
            ));
            return;
        }
        e.pos += n;
        h.grown = true;
    }

    /// The session's lease of `pl` that `release`, `grow` and `preempt
    /// leases by` act on, when it has several (a leg's passed to it): the
    /// earliest.
    fn lease_of(&self, sid: usize, pl: usize) -> Option<usize> {
        self.sessions[sid]
            .leases
            .iter()
            .position(|l| l.pool == pl && !l.suspended)
    }

    /// The lease of `pl` a tick set aside for `sid` (`suspend`), if any.
    fn set_aside_of(&self, sid: usize, pl: usize) -> Option<usize> {
        self.sessions[sid]
            .leases
            .iter()
            .position(|l| l.pool == pl && l.suspended)
    }

    /// What a `grow` of `pl` enlarges for `sid`: the innermost hold of
    /// it, or, outside every hold of it, the session's lease of it.
    fn grown(&self, sid: usize, pl: usize) -> Option<Grown> {
        self.sessions[sid]
            .innermost(pl)
            .map(|(hi, k)| Grown::Hold(hi, k))
            .or_else(|| self.lease_of(sid, pl).map(Grown::Lease))
    }

    fn grown_alloc(&self, sid: usize, g: Grown) -> f64 {
        match g {
            Grown::Hold(hi, k) => self.sessions[sid].holds[hi].pools[k].alloc,
            Grown::Lease(i) => self.sessions[sid].leases[i].alloc,
        }
    }

    /// Whether `need` more units of `pl` fit for `sid`'s allocation `g`.
    fn fits_grown(&self, pl: usize, need: f64, sid: usize, g: Grown) -> bool {
        let own = match g {
            Grown::Hold(hi, k) => Some((sid, hi, k)),
            Grown::Lease(_) => None,
        };
        self.fits_for(pl, need, own)
    }

    fn add_alloc(&mut self, sid: usize, pl: usize, g: Grown, need: f64) {
        self.make_room(pl, need);
        self.pools[pl].used += need;
        match g {
            Grown::Hold(hi, k) => self.sessions[sid].holds[hi].pools[k].alloc += need,
            Grown::Lease(i) => self.sessions[sid].leases[i].alloc += need,
        }
    }

    /// `shrink P (n)`: give back `n` units of the innermost hold of `sid`
    /// on `pl`, or of its lease of `pl` outside every hold, rounded down to
    /// blocks. A hold keeps at least what it has computed; a lease or a
    /// hold given back to nothing stays, at nothing. Nothing held or leased
    /// there fails the run, as `grow` does.
    fn shrink(&mut self, sid: usize, pl: usize, units: f64) {
        let name = &self.p.pools[pl].name;
        let Some(g) = self.grown(sid, pl) else {
            self.error = Some(nothing_held("shrink", name));
            return;
        };
        let alloc_now = self.grown_alloc(sid, g);
        // a hold keeps its position, a lease something: a lease of nothing
        // is `release`'s, and would read as one that has ended
        let floor = match g {
            Grown::Hold(hi, k) => self.sessions[sid].holds[hi].pools[k].pos,
            Grown::Lease(_) => 0.0,
        };
        let target = self.round_up(pl, (alloc_now - units).max(floor));
        if matches!(g, Grown::Lease(_)) && target <= 0.0 {
            self.error = Some(format!(
                "`shrink {name}` would leave the session's lease of `{name}` at nothing; \
                 `release {name}` ends it"
            ));
            return;
        }
        let back = alloc_now - target;
        if back <= 0.0 {
            return;
        }
        self.pools[pl].used -= back;
        match g {
            Grown::Hold(hi, k) => {
                // under `reserve held`, what it gives back is no longer reserved either
                let e = &mut self.sessions[sid].holds[hi].pools[k];
                e.alloc -= back;
                e.reserved = (e.reserved - back).max(e.alloc);
            }
            Grown::Lease(i) => {
                let l = &mut self.sessions[sid].leases[i];
                l.alloc -= back;
                l.computed = l.computed.min(l.alloc);
            }
        }
        self.retry_growers(pl);
    }

    /// Allocate `units` more for the innermost hold of `sid` on `pl`, or
    /// for its lease of `pl` outside every hold of it. Returns false if the
    /// session blocked (or its hold was preempted); a lease the growth
    /// preempts is the session's no more, and the session goes on.
    fn grow(&mut self, sid: usize, pl: usize, units: f64) -> bool {
        let Some(g) = self.grown(sid, pl) else {
            self.error = Some(nothing_held("grow", &self.p.pools[pl].name));
            return false;
        };
        let alloc_now = self.grown_alloc(sid, g);
        let target = self.round_up(pl, alloc_now + units);
        let need = target - alloc_now;
        if need <= 0.0 {
            return true;
        }
        if matches!(g, Grown::Lease(_)) && target > self.pools[pl].cap + EPS {
            // ending every lease would not make room: the growth would end
            // its own and go on with nothing, unseen
            let name = &self.p.pools[pl].name;
            self.error = Some(format!(
                "`grow {name}`: the lease would hold {target} of a cap of {}; no \
                 preemption makes room for it",
                self.pools[pl].cap
            ));
            return false;
        }
        if let (Grown::Lease(_), Preempt::Leases { every: Some(_), .. }) =
            (g, &self.p.pools[pl].preempt)
        {
            // the pool acts at its ticks: the growth is let through now,
            // past the cap if it must, where no guard has passed
            self.evict_for(pl, need);
            self.pools[pl].used += need;
            if let Grown::Lease(i) = g {
                self.sessions[sid].leases[i].alloc += need;
            }
            self.schedule_tick(pl);
            return true;
        }
        loop {
            if self.fits_grown(pl, need, sid, g) {
                self.add_alloc(sid, pl, g, need);
                return true;
            }
            let p = self.p;
            let victim = match &p.pools[pl].preempt {
                Preempt::None => None,
                Preempt::By { keys, .. } => self.victim(pl, keys, self.candidates(pl)),
                Preempt::Leases { keys, .. } => self.victim(pl, keys, self.lessees(pl)),
            };
            match victim {
                Some(victim) if p.pools[pl].preempt.preempts_holds() => {
                    self.preempt(victim, pl);
                    if victim == sid {
                        return false;
                    }
                }
                Some(victim) => {
                    self.revoke(victim, pl);
                    if victim == sid {
                        // its own lease is gone: nothing left to grow
                        return true;
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

    /// Who a growth on `pl` may preempt, in admission order, so that the
    /// last is vLLM's `running[-1]` (scheduler.py:742-813): the holders of
    /// `pl` that are residents of a step stage whose memory `pl` is, by the
    /// session's latest admission, which is the residents' serving order
    /// (`running` is in order of scheduling, and a request that queued once
    /// more for a slot after its KV arrived took its place then, not when
    /// its blocks were allocated). A holder that has left the engine is not
    /// a candidate: a prefiller's finished request keeps its blocks leased
    /// for the decoder's read and is in no `running` list, and a decoder's
    /// request waiting for that read (`WAITING_FOR_REMOTE_KVS`) holds its
    /// blocks and is not in `running` either; with no resident holding the
    /// pool there is nobody to preempt and the grower waits. A pool that is
    /// no engine's memory: its holders that hold it in a scope (a lease is
    /// not preempted), in the order the pool admitted them.
    fn candidates(&self, pl: usize) -> Vec<usize> {
        let engines: Vec<usize> = self
            .p
            .stages
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(&s.kind, CStageKind::Step(st) if st.memory == Some(pl)))
            .map(|(i, _)| i)
            .collect();
        let holders = self.pools[pl].holders.iter().copied();
        if engines.is_empty() {
            // a session holding the pool twice (a nested hold, or a hold and
            // a lease) is one candidate, at its first place
            let mut c: Vec<usize> = vec![];
            for s in holders.filter(|&s| self.sessions[s].innermost(pl).is_some()) {
                if !c.contains(&s) {
                    c.push(s);
                }
            }
            return c;
        }
        let mut c: Vec<usize> = holders
            .filter(|&s| {
                matches!(self.sessions[s].status, Status::InStage(x, _) if engines.contains(&x))
            })
            .collect();
        c.sort_by_key(|&s| self.sessions[s].adm_seq);
        c.dedup();
        c
    }

    /// `preempt leases by`: the sessions that lease `pl` and hold it in no
    /// scope, in the order the pool admitted them.
    fn lessees(&self, pl: usize) -> Vec<usize> {
        let mut c: Vec<usize> = vec![];
        for &s in &self.pools[pl].holders {
            let ss = &self.sessions[s];
            if ss.innermost(pl).is_none()
                && ss.leases.iter().any(|l| l.pool == pl && !l.suspended)
                && !c.contains(&s)
            {
                c.push(s);
            }
        }
        c
    }

    /// `preempt by (keys)`: the candidate with the least keys, read for
    /// each (`Moment::Victim`: its attributes, `admission`, its place in
    /// the candidates' admission order, `decoding`, and its `position` on
    /// `pl`), ties to the one admitted last. `admission` is the place, not
    /// the session's sequence number, so that `preempt lifo`, `by
    /// (-admission)`, is the last candidate on a pool that is no engine's
    /// memory too, where the order is the pool's and a session's latest
    /// admission may have been to another pool.
    fn victim(&mut self, pl: usize, keys: &[CExpr], candidates: Vec<usize>) -> Option<usize> {
        let mut best: Option<(KeyOrd, usize)> = None;
        for (place, s) in candidates.into_iter().enumerate() {
            let position = match self.sessions[s].innermost(pl) {
                Some((hi, k)) => self.sessions[s].holds[hi].pools[k].pos,
                None => self.sessions[s]
                    .leases
                    .iter()
                    .rev()
                    .find(|l| l.pool == pl)
                    .map_or(0.0, |l| l.computed),
            };
            let decoding = match self.sessions[s].status {
                Status::InStage(st, j) => self.stages[st]
                    .jobs
                    .get(&j)
                    .is_some_and(|job| job.mode == RunMode::Decode),
                _ => false,
            };
            let ctx = Ctx {
                sid: Some(s),
                admission: place as f64,
                decoding: if decoding { 1.0 } else { 0.0 },
                position,
                ..Default::default()
            };
            let key = KeyOrd(
                keys.iter()
                    .map(|k| self.eval(k, &ctx, Which::Session))
                    .collect(),
            );
            if best.as_ref().is_none_or(|(old, _)| key <= *old) {
                best = Some((key, s));
            }
        }
        best.map(|(_, s)| s)
    }

    fn retry_growers(&mut self, pl: usize) {
        while let Some(&sid) = self.pools[pl].growers.front() {
            let Status::Growing(_, units, resume) = self.sessions[sid].status else {
                self.pools[pl].growers.pop_front();
                continue;
            };
            let g = self
                .grown(sid, pl)
                .expect("a growing session holds or leases the pool");
            let alloc_now = self.grown_alloc(sid, g);
            let need = self.round_up(pl, alloc_now + units) - alloc_now;
            if !self.fits_grown(pl, need, sid, g) {
                break;
            }
            self.pools[pl].growers.pop_front();
            self.add_alloc(sid, pl, g, need);
            match resume {
                // a growing job: the growth covers the tokens it stalled
                // on, which move the position when they are computed (#482)
                Some((st, j)) => self.sessions[sid].status = Status::InStage(st, j),
                None => {
                    self.sessions[sid].status = Status::Ready;
                    self.ready.push_back(sid);
                }
            }
        }
    }

    /// `(allocated, position)` of the innermost hold of `sid` on `pl`.
    fn hold_alloc_pos(&mut self, sid: usize, pl: usize) -> (f64, f64) {
        let Some((hi, k)) = self.sessions[sid].innermost(pl) else {
            self.error = Some(format!(
                "`growing {}` outside a hold of it",
                self.p.pools[pl].name
            ));
            return (0.0, 0.0);
        };
        let e = &self.sessions[sid].holds[hi].pools[k];
        (e.alloc, e.pos)
    }

    /// Put the innermost hold of `sid` on `pl` at position `pos`.
    fn set_pos(&mut self, sid: usize, pl: usize, pos: f64) {
        let (hi, k) = self.sessions[sid]
            .innermost(pl)
            .expect("a growing run is inside a hold of its pool");
        let h = &mut self.sessions[sid].holds[hi];
        h.pools[k].pos = pos;
        h.grown = true;
    }

    /// Units of `pl` held by `sid` (all its holds).
    fn held_in(&self, sid: usize, pl: usize) -> f64 {
        self.sessions[sid]
            .holds
            .iter()
            .flat_map(|h| h.pools.iter())
            .filter(|e| e.pool == pl)
            .map(|e| e.alloc)
            .sum()
    }

    /// Preempt `victim`'s hold on `pl`: its job leaves its stage, the
    /// hold's allocations are released (cached), and it re-queues at the
    /// head of the pool's queue with the hold to execute again.
    fn preempt(&mut self, victim: usize, pl: usize) {
        let (hi, k) = self.sessions[victim]
            .innermost(pl)
            .expect("victim holds the pool");
        // what the hold has computed on this pool: its position there, which
        // starts at the cached prefix it consumed and advances with its
        // `growing` runs. Not the allocation: a holder preempted before its
        // first iteration has computed nothing of what it was allocated
        // (vLLM: `num_computed_tokens` is 0 for it, and it has no output)
        let computed = self.sessions[victim].holds[hi].pools[k].pos;
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
        self.log_who(victim, |who| EventKind::Preempt { who, pool: pl });
        if self.sessions[victim].holds[hi].request {
            // an engine's request: no body to run again. It keeps the tokens
            // it has and what it generated, caches what it computed, and asks
            // again. A decode has sampled a token for each one it computed:
            // the tokens it has are its position and the one sampled there
            if let Some(r) = self.sessions[victim].req.as_mut() {
                r.known = r.known.max(computed + 1.0);
                r.generated = r.known - r.input;
                r.preempted = true;
            }
            self.end_parts(victim);
            let hi = self.sessions[victim]
                .holds
                .iter()
                .rposition(|h| h.request)
                .expect("the request's hold");
            let h = self.sessions[victim].holds.remove(hi);
            for e in &h.pools {
                let read = (&h.chain[..], e.shared, &e.path[..]);
                self.release_units(victim, e.pool, e.alloc, e.pos, h.cache, read);
            }
            self.pools[pl].preemptions += 1;
            self.preempted = true;
            self.preempted_now += 1.0;
            let tail = matches!(self.p.pools[pl].preempt, Preempt::By { tail: true, .. });
            self.request_ask(victim, !tail);
            return;
        }
        // unwind holds inner to `hi` (nested holds), then `hi` itself. A
        // preempted hold caches what it computed, its position, not its
        // allocation: the scope's end counts a hold without a `growing` run
        // as having computed what it holds, but a preemption cuts the body
        // short. vLLM caches what it schedules (kv_cache_manager.py:602-606)
        // and counts it computed right after (`_update_after_schedule`,
        // scheduler.py:1584-1597), and its victim, `running.pop()`, is one
        // this step has not scheduled (scheduler.py:742-813): what stays
        // cached is the full blocks of `num_computed_tokens`, the position
        while self.sessions[victim].holds.len() > hi {
            let h = self.sessions[victim].holds.pop().unwrap();
            for e in &h.pools {
                let read = (&h.chain[..], e.shared, &e.path[..]);
                self.release_units(victim, e.pool, e.alloc, e.pos, h.cache, read);
            }
            // pop frames down to and including that hold's frame
            while let Some(f) = self.sessions[victim].frames.pop() {
                if f.kind == FrameKind::Hold && f.block == h.body {
                    break;
                }
            }
        }
        let h_pools = {
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
            self.wanted(pools, victim)
        };
        let lease_pool = {
            let parent = self.sessions[victim].frames.last().cloned().unwrap();
            let p = self.p;
            match &p.blocks[parent.block][parent.pc - 1] {
                CStmt::Hold {
                    lease: Some((r, t)),
                    ..
                } => Some((self.session_index(r, victim), Expiry::After(t))),
                _ => None,
            }
        };
        let (body, cache, prefix) = {
            let parent = self.sessions[victim].frames.last().cloned().unwrap();
            let p = self.p;
            match &p.blocks[parent.block][parent.pc - 1] {
                CStmt::Hold {
                    body,
                    cache,
                    reuse,
                    prefix,
                    ..
                } => (*body, (cache.as_ref(), reuse.as_ref()), prefix.as_deref()),
                _ => unreachable!(),
            }
        };
        let (cache, reuse) = cache;
        let chain = self.chain_of(prefix, victim);
        self.pools[pl].preemptions += 1;
        self.preempted = true;
        self.preempted_now += 1.0;
        let pending = Pending {
            pools: h_pools,
            reuse,
            cache,
            chain,
            lease: lease_pool,
            body,
            queued_at: self.now,
            request: false,
            part: false,
        };
        // back at the head (vLLM's `prepend_request`), or with `requeue
        // tail` at the back, a newcomer to the queue's keys and `waited`
        let tail = matches!(self.p.pools[pl].preempt, Preempt::By { tail: true, .. });
        self.enqueue_hold(victim, pending, !tail);
    }

    /// A growth on `pl` under `preempt leases by` takes `victim`'s lease
    /// of it, which `victim` holds in no scope: the lease ends, its units
    /// go back and `cache` applies, and the session, which is elsewhere (at
    /// a tool, at another queue, at an engine), goes on. `allocated` is then
    /// 0, and a `grow` of the pool fails the run: what the session does next
    /// is the program's. No session waits to grow a lease under this rule,
    /// since the grower is a candidate itself. ThunderAgent pauses an idle
    /// program this way (`docs/design/thunderagent.md`).
    fn revoke(&mut self, victim: usize, pl: usize) {
        let i = self.lease_of(victim, pl).expect("victim leases the pool");
        self.log_who(victim, |who| EventKind::Revoke { who, pool: pl });
        self.pools[pl].preemptions += 1;
        if !matches!(
            self.p.pools[pl].preempt,
            Preempt::Leases {
                suspend: Some(_),
                ..
            }
        ) {
            self.end_lease(victim, i);
            return;
        }
        // set aside (`suspend`): the prefix it read is let go, as at a
        // release, what no other hold reads is cached again, and the lease
        // keeps only its own units to come back with at a tick
        let serial = self.sessions[victim].serial;
        let l = &mut self.sessions[victim].leases[i];
        let (alloc, shared, path) = (l.alloc, l.shared, std::mem::take(&mut l.path));
        l.alloc -= shared;
        l.shared = 0.0;
        l.suspended = true;
        let (back, gone) = self.pools[pl].cache.unlock(&path, serial);
        self.pools[pl].used -= alloc - (shared - back - gone);
        self.pools[pl].cached += back;
        if let Some(h) = self.pools[pl].holders.iter().position(|&x| x == victim) {
            self.pools[pl].holders.remove(h);
        }
        self.schedule_tick(pl);
    }

    /// The next tick of `pl` under `preempt leases by … every (dt)`, at the
    /// first multiple of `dt` after now, unless one is due already.
    fn schedule_tick(&mut self, pl: usize) {
        let Preempt::Leases {
            every: Some(dt), ..
        } = self.p.pools[pl].preempt
        else {
            return;
        };
        if self.pools[pl].next_tick.is_some() {
            return;
        }
        let mut k = (self.now / dt).floor() as u64 + 1;
        if k as f64 * dt <= self.now {
            k += 1;
        }
        self.pools[pl].next_tick = Some(k);
        self.at(k as f64 * dt, Ev::PoolTick { pool: pl });
    }

    /// A tick of `pl`: under `suspend by`, the leases set aside come back
    /// in its keys' order, each that fits; then, while the pool is over its
    /// cap, the lease with the least keys ends, or is set aside. The next
    /// tick is due while the pool has a lease.
    fn pool_tick(&mut self, pl: usize) {
        let p = self.p;
        let Preempt::Leases {
            keys,
            every: Some(dt),
            suspend,
        } = &p.pools[pl].preempt
        else {
            return;
        };
        let k = self.pools[pl].next_tick.take().expect("a tick was due");
        if let Some(back_keys) = suspend {
            let mut aside: Vec<usize> = vec![];
            for (s, ss) in self.sessions.iter().enumerate() {
                // one that holds the pool anew has what it needs there
                if ss.status != Status::Ended
                    && ss.leases.iter().any(|l| l.pool == pl && l.suspended)
                    && !ss.leases.iter().any(|l| l.pool == pl && !l.suspended)
                    && ss.innermost(pl).is_none()
                    && !aside.contains(&s)
                {
                    aside.push(s);
                }
            }
            // in admission order, as `lessees` is: `admission` reads it
            aside.sort_by_key(|&s| self.sessions[s].adm_seq);
            for s in self.in_order(pl, back_keys, aside) {
                let Some(i) = self.set_aside_of(s, pl) else {
                    continue;
                };
                let alloc = self.sessions[s].leases[i].alloc;
                if self.pools[pl].used + alloc > self.pools[pl].cap + EPS {
                    continue;
                }
                // cached units give way, as to any admission
                self.make_room(pl, alloc);
                self.pools[pl].used += alloc;
                self.pools[pl].holders.push(s);
                self.sessions[s].leases[i].suspended = false;
            }
        }
        while self.pools[pl].used > self.pools[pl].cap + EPS {
            let Some(v) = self.victim(pl, keys, self.lessees(pl)) else {
                break;
            };
            self.revoke(v, pl);
        }
        // what was let go is cached only within the cap
        self.evict_for(pl, 0.0);
        let leased = self
            .sessions
            .iter()
            .any(|ss| ss.status != Status::Ended && ss.leases.iter().any(|l| l.pool == pl));
        // (a lease set aside in this tick may have scheduled the next)
        if leased && self.pools[pl].next_tick.is_none() {
            self.pools[pl].next_tick = Some(k + 1);
            self.at((k + 1) as f64 * dt, Ev::PoolTick { pool: pl });
        }
        self.try_admit_all();
    }

    /// The next tick of `pl` under `queue … every (dt)`, at the first
    /// multiple of `dt` after now, unless one is due already.
    fn schedule_queue_tick(&mut self, pl: usize) {
        let Some(dt) = self.p.pools[pl].queue_every else {
            return;
        };
        if self.pools[pl].next_queue_tick.is_some() {
            return;
        }
        let mut k = (self.now / dt).floor() as u64 + 1;
        if k as f64 * dt <= self.now {
            k += 1;
        }
        self.pools[pl].next_queue_tick = Some(k);
        self.at(k as f64 * dt, Ev::QueueTick { pool: pl });
    }

    /// A tick of `pl` under `queue … every`: the holds that waited when it
    /// began are tried in the queue's order, each admitted one running
    /// until it blocks before the next is tried, one that does not fit
    /// passed over. A hold queued during the tick waits for the next.
    fn queue_tick(&mut self, pl: usize) {
        let dt = self.p.pools[pl].queue_every.expect("a queue tick");
        let k = self.pools[pl]
            .next_queue_tick
            .take()
            .expect("a tick was due");
        let mut waiting: Vec<usize> = self.pools[pl].queue.iter().map(|e| e.sid).collect();
        while let Some((index, sid)) = self.next_waiter_among(pl, Some(&waiting)) {
            waiting.retain(|&s| s != sid);
            let pending = self.pending_now(sid);
            if !self.fits_all(&pending) {
                continue;
            }
            self.pools[pl].queue.remove(index);
            self.admit(sid, pending);
            self.run_ready();
            if self.error.is_some() {
                return;
            }
        }
        // (a hold queued in this tick may have scheduled the next)
        if !self.pools[pl].queue.is_empty() && self.pools[pl].next_queue_tick.is_none() {
            self.pools[pl].next_queue_tick = Some(k + 1);
            self.at((k + 1) as f64 * dt, Ev::QueueTick { pool: pl });
        }
    }

    /// `candidates` in the order of `keys`, the least first (`victim`'s
    /// order, read at the same moment).
    fn in_order(&mut self, pl: usize, keys: &[CExpr], mut candidates: Vec<usize>) -> Vec<usize> {
        let mut out = vec![];
        while !candidates.is_empty() {
            let Some(v) = self.victim(pl, keys, candidates.clone()) else {
                break;
            };
            candidates.retain(|&c| c != v);
            out.push(v);
        }
        out
    }

    // -------------------------------------------------------- stages ----

    /// A job at stage `st`: it waits `wait` seconds there first, present
    /// and taking no capacity (a run's `latency`), then its work starts.
    fn start_job(
        &mut self,
        st: usize,
        owner: Option<usize>,
        work: f64,
        mode: RunMode,
        growing: Option<usize>,
        wait: f64,
    ) {
        let id = self.next_job;
        self.next_job += 1;
        let now = self.now;
        if work <= 0.0 && wait <= 0.0 {
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
            self.start_flow(vec![st], owner, work, wait);
            return;
        }
        if let Some(sid) = owner {
            self.sessions[sid].status = Status::InStage(st, id);
            self.log_who(sid, |who| EventKind::Run {
                who,
                stages: vec![st],
                mode,
            });
        }
        self.stages[st].jobs.insert(
            id,
            Job {
                owner,
                work,
                mode,
                growing,
                enqueued: now,
                started: None,
            },
        );
        if wait > 0.0 {
            self.at(now + wait, Ev::Start { stage: st, job: id });
            return;
        }
        self.begin_job(st, id);
    }

    /// The work of job `id`, in `st`'s jobs since its run began, starts
    /// now: it joins the stage's queue, its share or its residents. Work of
    /// none (a transfer of zero bytes after its wait) is done at once.
    fn begin_job(&mut self, st: usize, id: u64) {
        let now = self.now;
        let stage = &mut self.stages[st];
        let work = stage.jobs[&id].work;
        if work <= 0.0 {
            // served now, for nothing: its wait was its run's `latency`
            let mut job = stage.jobs.remove(&id).unwrap();
            job.started = Some(now);
            self.job_done(st, job);
            return;
        }
        match &mut stage.kind {
            Kind::Fifo { queue, .. } => {
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
                let job = stage.jobs.get_mut(&id).unwrap();
                job.work = tag;
                job.started = Some(now);
                set.insert((OrdF64(tag), id));
                *dirty = true;
            }
            Kind::Delay => {
                stage.jobs.get_mut(&id).unwrap().started = Some(now);
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
                let job = stage.jobs.get_mut(&id).unwrap();
                job.started = Some(now);
                let owner = job.owner;
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

    /// A run's wait is over: the job starts its work, if it is still there.
    fn on_start(&mut self, st: usize, id: u64) {
        if !self.stages[st].jobs.contains_key(&id) {
            return;
        }
        if matches!(self.stages[st].kind, Kind::Shared { .. }) {
            self.flows_advance();
            let now = self.now;
            let Some(f) = self.flows.get(&id) else {
                return;
            };
            let stages = f.stages.clone();
            for &s in &stages {
                if let Some(job) = self.stages[s].jobs.get_mut(&id) {
                    job.started = Some(now);
                }
            }
            if f.remaining <= 0.0 {
                self.flow_done(id);
                return;
            }
            self.flows_touched.extend(&stages);
            self.flows_dirty = true;
            return;
        }
        self.begin_job(st, id);
    }

    // ------------------------------------------------ shared stages ----

    /// A job on shared stages: it holds every one of `stages` until its work
    /// is done at the rate the program's `share` gives it, after `wait`
    /// seconds at them taking no share (a run's `latency`).
    fn start_flow(&mut self, stages: Vec<usize>, owner: Option<usize>, work: f64, wait: f64) {
        let id = self.next_job;
        self.next_job += 1;
        let now = self.now;
        if work <= 0.0 && wait <= 0.0 {
            if let Some(sid) = owner {
                self.sessions[sid].status = Status::Ready;
                self.ready.push_back(sid);
            }
            return;
        }
        if let Some(sid) = owner {
            self.sessions[sid].status = Status::InStage(stages[0], id);
            self.log_who(sid, |who| EventKind::Run {
                who,
                stages: stages.clone(),
                mode: RunMode::Plain,
            });
        }
        self.flows_advance();
        let waiting = wait > 0.0;
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
                    started: (!waiting).then_some(now),
                },
            );
        }
        if waiting {
            // present at its stages, at rate 0 until its wait is over
            self.at(
                now + wait,
                Ev::Start {
                    stage: stages[0],
                    job: id,
                },
            );
            self.flows.insert(
                id,
                Flow {
                    stages,
                    remaining: work,
                    rate: 0.0,
                },
            );
            return;
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

    /// Whether flow `id` is past its wait: a waiting flow is present at its
    /// stages, with no share of them (`Job::started` is `None`).
    fn flow_started(&self, id: u64) -> bool {
        let s = self.flows[&id].stages[0];
        self.stages[s].jobs[&id].started.is_some()
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

    /// The rates of every flow under its fabric's `share`, and the earliest
    /// end among them scheduled; an end scheduled before is stale.
    fn flows_reschedule(&mut self) {
        self.flows_dirty = false;
        self.flows_advance();
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
        // a fabric has one policy (`Program::stage_shares`), so the flows of
        // a component are all of it, and the touched components of each
        // policy are solved apart from the others'
        let policy = |st: &StageState| match st.kind {
            Kind::Shared { share, .. } => share,
            _ => unreachable!("a flow holds shared stages only"),
        };
        for share in crate::ir::Share::ALL.map(|(_, s)| s) {
            let ids: Vec<u64> = ids
                .iter()
                .copied()
                // a flow still in its wait takes no share
                .filter(|id| {
                    policy(&self.stages[self.flows[id].stages[0]]) == share
                        && self.flow_started(*id)
                })
                .collect();
            let comp_stages: Vec<usize> = comp_stages
                .iter()
                .copied()
                .filter(|&s| policy(&self.stages[s]) == share)
                .collect();
            if ids.is_empty() {
                continue;
            }
            self.flows_solve(share, &ids, &comp_stages);
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

    /// The rates of the flows `ids`, all of policy `policy`, over the stages
    /// `comp_stages` they hold.
    fn flows_solve(&mut self, policy: crate::ir::Share, ids: &[u64], comp_stages: &[usize]) {
        let cap = |st: &StageState| match st.kind {
            Kind::Shared { cap, .. } => cap,
            _ => unreachable!("a flow holds shared stages only"),
        };
        match policy {
            crate::ir::Share::Bottleneck => {
                // each flow's equal share at the tightest of its stages,
                // among the flows past their wait
                let sharing =
                    |st: &StageState| st.jobs.values().filter(|j| j.started.is_some()).count();
                for &id in ids {
                    let r = self.flows[&id]
                        .stages
                        .iter()
                        .map(|&s| cap(&self.stages[s]) / sharing(&self.stages[s]) as f64)
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
                for &s in comp_stages {
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
            self.log_who(sid, |who| EventKind::Done { who, stage: st });
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
        self.preempted_now = 0.0;
        // each served prefill's attention work, summed over those still in
        // the batch when the iteration is costed
        let mut attn_by: Vec<(u64, f64)> = vec![];
        // a body's `set`s take effect with its iteration (below)
        let regs_before = spec.iteration.as_ref().map(|_| self.regs.clone());
        let mut admitted_any = false;
        if let Some(body) = &spec.iteration {
            let mut plan = Plan {
                st,
                spec,
                chunk,
                left: budget,
                served: BTreeSet::new(),
                assign: vec![],
                attn_by: vec![],
                admitted: 0.0,
                refused: false,
            };
            self.run_body(&mut plan, body);
            if self.error.is_some() {
                return;
            }
            assign = plan.assign;
            attn_by = plan.attn_by;
            admitted_any = plan.admitted > 0.0;
        } else {
            // a prefill the granule refused ends the admissions
            let mut refused = false;
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
                    if left > 0.0 && !preempted && !refused && self.admit_bound(st, admit_left) {
                        continue;
                    }
                    break;
                };
                served.insert(id);
                let rule = exclusive.then_some(Exclusive {
                    resident_prefill,
                    budget,
                });
                match self.give(st, id, chunk, rule, &mut left, &mut assign, &mut attn_by) {
                    Give::Skipped => continue,
                    Give::Refused => {
                        refused = true;
                        continue;
                    }
                    // preempted itself (lifo): vLLM stops serving the running
                    // requests for this step (scheduler.py:807-813, `break`).
                    // In admission order the grower is then the last resident
                    // anyway; under `serve by` it need not be.
                    Give::Stopped => {
                        if self.error.is_some() {
                            return;
                        }
                        break;
                    }
                    Give::Gave(mode) => {
                        if left <= 0.0 || (exclusive && mode == RunMode::Prefill) {
                            // A selected prefill is a lone batch. In particular do
                            // not admit another waiting request with its leftover
                            // budget.
                            break;
                        }
                    }
                }
            }
        }
        // Growth can preempt an earlier candidate. Only surviving, selected
        // jobs advance their holds' computed positions, by their tokens: a
        // run's tokens are whole (#661), so its chunks sum to its work.
        assign.retain(|(id, _)| self.stages[st].jobs.contains_key(id));
        for &(id, tokens) in &assign {
            let j = &self.stages[st].jobs[&id];
            if let (Some(pl), Some(sid)) = (j.growing, j.owner) {
                let (_, pos) = self.hold_alloc_pos(sid, pl);
                self.set_pos(sid, pl, pos + tokens);
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
            // work it did not schedule: waiting for an event, which the
            // report names if none came (#263)
            let work = !self.residents(st).is_empty() || self.bound_waiting(st);
            self.stages[st].idle_with_work = work;
            // no iteration: what its body set did not happen either, unless
            // the try admitted someone, which stays, and the sets with it
            if let Some(regs) = regs_before
                && !admitted_any
            {
                self.regs = regs;
            }
            return;
        }
        self.stages[st].idle_with_work = false;
        let ntok: f64 = assign.iter().map(|a| a.1).sum();
        let attn: f64 = attn_by
            .iter()
            .filter(|(j, _)| assign.iter().any(|a| a.0 == *j))
            .map(|(_, a)| a)
            .sum();
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
                "engine `{}`: its `worker` is {cost} for an iteration of {ntok} tokens; an iteration that \
                 schedules tokens lasts a positive time",
                p.stages[st].name
            ));
            return;
        }
        self.check_iteration(st, chunk, &ctx);
        self.served[st] += ntok;
        if self.log.is_some() {
            // each request with its tokens, in one pass so the two lists
            // line up (every job is one: no spill goes via an engine,
            // #733, so `each` sums to `tokens`); the memory pool once the
            // iteration is scheduled, in units
            let (reqs, each): (Vec<Who>, Vec<f64>) = assign
                .iter()
                .map(|&(id, t)| {
                    let sid = self.stages[st].jobs[&id]
                        .owner
                        .expect("validation: a job on an engine is a request's");
                    (self.who(sid), t)
                })
                .unzip();
            let memory = spec
                .memory
                .map(|pl| (self.pools[pl].used, self.pools[pl].cached));
            self.log(EventKind::Iter {
                stage: st,
                dur: cost,
                tokens: ntok,
                budget,
                reqs,
                each,
                memory,
            });
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

    /// Give resident `id` its tokens in the iteration being planned: one for
    /// a decode, up to `chunk` for a prefill, no more than is `left`; a
    /// `growing` job first grows its hold to the position it will reach,
    /// which may preempt. `rule` is `exclusive prefill`'s, under which a
    /// prefill takes the whole budget and the decodes it displaces give
    /// theirs back.
    #[allow(clippy::too_many_arguments)]
    fn give(
        &mut self,
        st: usize,
        id: u64,
        chunk: f64,
        rule: Option<Exclusive>,
        left: &mut f64,
        assign: &mut Vec<(u64, f64)>,
        attn_by: &mut Vec<(u64, f64)>,
    ) -> Give {
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
        let blocked = rule.is_some_and(|r| r.resident_prefill) && mode == RunMode::Decode;
        let available = if let Some(r) = rule
            && mode == RunMode::Prefill
        {
            r.budget
        } else {
            *left
        };
        let mut tokens = if blocked { 0.0 } else { want.min(available) };
        // `granule g`: a prefill short of its remainder takes a multiple of
        // `g` (none under `inf`: whole or nothing)
        if mode == RunMode::Prefill
            && tokens < remaining
            && let CStageKind::Step(spec) = &self.p.stages[st].kind
            && let Some(CExpr::Num(g)) = &spec.granule
        {
            tokens = if g.is_finite() {
                (tokens / g).floor() * g
            } else {
                0.0
            };
            if tokens <= 0.0 {
                return Give::Refused;
            }
        }
        if tokens <= 0.0 {
            return Give::Skipped;
        }
        // growth before the tokens are committed: the hold must cover
        // the sequence position after this iteration (vLLM
        // `allocate_slots`), block by block
        if let (Some(pl), Some(sid)) = (growing, owner) {
            if matches!(self.sessions[sid].status, Status::Growing(..)) {
                // stalled from an earlier iteration: no tokens
                return Give::Skipped;
            }
            let (alloc, pos) = self.hold_alloc_pos(sid, pl);
            if self.error.is_some() {
                return Give::Stopped;
            }
            let need = pos + tokens - alloc;
            let grew = need <= EPS || self.grow(sid, pl, need);
            // The growth may have preempted a resident this iteration has
            // already served: under `serve by` the latest admitted, the
            // victim, need not be the last served. It leaves the batch
            // and its tokens return to the budget (vLLM's PRIORITY path,
            // scheduler.py:779-797, which keeps the victim apart from the
            // visiting order as serQ does).
            let jobs = &self.stages[st].jobs;
            assign.retain(|&(j, t)| {
                let keep = jobs.contains_key(&j);
                if !keep {
                    *left += t;
                }
                keep
            });
            if !grew {
                // waiting (none): stalls as a resident, no tokens, and the
                // next resident is served
                if matches!(self.sessions[sid].status, Status::Growing(..)) {
                    return Give::Skipped;
                }
                // preempted itself
                return Give::Stopped;
            }
            if mode == RunMode::Prefill {
                attn_by.push((id, tokens * (pos + tokens / 2.0)));
            }
            // The computed position advances when the iteration is
            // settled, below, for the residents still in it: a resident
            // preempted later in this iteration keeps the position it had
            // (vLLM advances `num_computed_tokens` after `schedule`,
            // `_update_after_schedule`, scheduler.py:1584-1597), and exclusive-prefill
            // decodes are candidates until waiting admission has finished.
        } else if mode == RunMode::Prefill {
            attn_by.push((id, tokens * tokens / 2.0));
        }
        if let Some(r) = rule
            && mode == RunMode::Prefill
        {
            // Keep any allocation made for displaced decodes, as RBLN
            // keeps pending runner block deltas; cancel only their work.
            assign.clear();
            *left = r.budget;
        }
        assign.push((id, tokens));
        *left -= tokens;
        Give::Gave(mode)
    }

    /// Run a step stage's iteration body (`iteration { … }`) on the
    /// iteration being planned. Each statement runs once where it is
    /// written; a resident is served at most once in the iteration.
    fn run_body(&mut self, plan: &mut Plan<'p>, body: &'p [CIter]) {
        for s in body {
            if self.error.is_some() {
                return;
            }
            match s {
                CIter::Serve { only, by } => {
                    let keys = match by {
                        Some(keys) => keys.as_slice(),
                        None => match &plan.spec.serve {
                            CServe::By(keys) => keys.as_slice(),
                            CServe::ExclusivePrefill => unreachable!("refused with a body"),
                        },
                    };
                    // the residents `only` reads as 0 here: unserved, a
                    // later `serve` may take them
                    let mut skipped: BTreeSet<u64> = BTreeSet::new();
                    while plan.left > 0.0 {
                        let order = self.order_by(plan.st, keys, plan.spec.memory);
                        let Some(id) = order
                            .into_iter()
                            .find(|j| !plan.served.contains(j) && !skipped.contains(j))
                        else {
                            break;
                        };
                        if let Some(p) = only
                            && !self.serves(plan.st, id, p, plan.spec.memory)
                        {
                            skipped.insert(id);
                            continue;
                        }
                        plan.served.insert(id);
                        let given = self.give(
                            plan.st,
                            id,
                            plan.chunk,
                            None,
                            &mut plan.left,
                            &mut plan.assign,
                            &mut plan.attn_by,
                        );
                        match given {
                            Give::Stopped => break,
                            Give::Refused => plan.refused = true,
                            _ => {}
                        }
                    }
                }
                CIter::Admit { only, gate } => {
                    'admit: while plan.left > 0.0 && !plan.refused {
                        if let Some(g) = gate
                            && !self.guard(g, plan, "`admit waiting while (…)`")
                        {
                            break;
                        }
                        let before: BTreeSet<u64> = self.residents(plan.st).into_iter().collect();
                        if !self.admit_bound(plan.st, plan.left) {
                            break;
                        }
                        plan.admitted += 1.0;
                        // the newcomer, in the stage's order: what it brought
                        // to the engine, served now with the budget left
                        let newcomers: Vec<u64> = self
                            .serving_order(plan.st, plan.spec)
                            .into_iter()
                            .filter(|j| !before.contains(j) && !plan.served.contains(j))
                            .collect();
                        for id in newcomers {
                            if plan.left <= 0.0 {
                                break 'admit;
                            }
                            // one `only` excludes is admitted and waits,
                            // unserved, as under the stage's `serve only`
                            if let Some(p) = only
                                && !self.serves(plan.st, id, p, plan.spec.memory)
                            {
                                continue;
                            }
                            plan.served.insert(id);
                            let given = self.give(
                                plan.st,
                                id,
                                plan.chunk,
                                None,
                                &mut plan.left,
                                &mut plan.assign,
                                &mut plan.attn_by,
                            );
                            match given {
                                Give::Stopped => break 'admit,
                                // the granule refused it: no one after it
                                Give::Refused => {
                                    plan.refused = true;
                                    break 'admit;
                                }
                                _ => {}
                            }
                        }
                    }
                }
                CIter::Branch(g, a, b) => {
                    let taken = if self.guard(g, plan, "an `if` in a `scheduler`") {
                        a
                    } else {
                        b
                    };
                    self.run_body(plan, taken);
                }
                CIter::Set(r, e) => {
                    let v = self.plan_value(e, plan);
                    if !v.is_finite() && self.error.is_none() {
                        self.error = Some(format!(
                            "engine `{}`: `set {}` read {v}; a register holds a finite number",
                            self.p.stages[plan.st].name, self.p.registers[*r].name
                        ));
                    }
                    self.regs[*r] = v;
                }
            }
        }
    }

    /// A guard of an iteration body, read on the residents as they stand
    /// and what the iteration has done so far (`Moment::Plan`): 1 or 0, and
    /// anything else fails the run, as a session's `if` does.
    fn guard(&mut self, e: &CExpr, plan: &Plan, what: &str) -> bool {
        let v = self.plan_value(e, plan);
        if v == 1.0 {
            true
        } else {
            if v != 0.0 && self.error.is_none() {
                self.error = Some(format!(
                    "engine `{}`: {what} read {v}; a test is 1 or 0",
                    self.p.stages[plan.st].name
                ));
            }
            false
        }
    }

    /// An expression of an iteration body (`Moment::Plan`), read on the
    /// residents as they stand and what the iteration has done so far.
    fn plan_value(&mut self, e: &CExpr, plan: &Plan) -> f64 {
        let mut ctx = self.resident_totals(plan.st, plan.spec.memory);
        ctx.ntok = plan.assign.iter().map(|a| a.1).sum();
        ctx.npre = plan
            .assign
            .iter()
            // every job in `assign` is a resident: `give` drops a victim from it
            .filter(|(j, _)| self.stages[plan.st].jobs[j].mode == RunMode::Prefill)
            .map(|a| a.1)
            .sum();
        ctx.admitted = plan.admitted;
        ctx.preempted = self.preempted_now;
        self.eval(e, &ctx, Which::Session)
    }

    /// Read the claims over the iterations of stage `st` as an iteration
    /// starts, with the cost's context `ctx`, `demand` (what the residents
    /// could take in it, `chunk` capping a prefill's), `served` and `arrived`.
    fn check_iteration(&mut self, st: usize, chunk: f64, ctx: &Ctx) {
        let p = self.p;
        let mine = |c: &crate::ir::Claim| c.kind.stage() == Some(st);
        if !p.claims.iter().any(mine) {
            return;
        }
        let mut demand = 0.0;
        for j in self.residents(st) {
            let job = &self.stages[st].jobs[&j];
            demand += if job.mode == RunMode::Decode {
                1.0f64.min(job.work)
            } else if chunk > 0.0 {
                job.work.min(chunk)
            } else {
                job.work
            };
        }
        let ctx = Ctx {
            demand,
            served: self.served[st],
            arrived: self.arrivals as f64,
            ..ctx.clone()
        };
        let now = self.now;
        for (k, c) in p.claims.iter().enumerate() {
            if !mine(c) || self.claims[k].out_of_scope.is_some() {
                continue;
            }
            let holds = self.eval(&c.expr, &ctx, Which::Session) != 0.0;
            let state = &mut self.claims[k];
            state.checked += 1;
            if !holds {
                state.failures += 1;
            }
            let found = match c.kind {
                ClaimKind::SomeIteration(_) => holds,
                _ => !holds,
            };
            if found && state.first.is_none() {
                state.first = Some(now);
            }
        }
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
            if !self.fits_all(&pending) {
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
        match &spec.serve {
            CServe::By(keys) => self.order_by(st, keys, spec.memory),
            CServe::ExclusivePrefill => self.residents(st),
        }
    }

    /// The residents in ascending `keys`, ties in admission order.
    fn order_by(&mut self, st: usize, keys: &[CExpr], memory: Option<usize>) -> Vec<u64> {
        let mut r = self.residents(st);
        if keys.is_empty() {
            return r;
        }
        let totals = self.resident_totals(st, memory);
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
        self.client_token(sid, decode);
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
            CExpr::Cost(_, x) => self.eval(x, ctx, w),
            CExpr::Num(x) => *x,
            CExpr::Reg(r) => self.regs[*r],
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
                CtxVar::Position => ctx.position,
                CtxVar::Admitted => ctx.admitted,
                CtxVar::Preempted => ctx.preempted,
                CtxVar::Demand => ctx.demand,
                CtxVar::Served => ctx.served,
                CtxVar::Arrived => ctx.arrived,
                CtxVar::Input
                | CtxVar::Output
                | CtxVar::Known
                | CtxVar::Hit
                | CtxVar::Loaded
                | CtxVar::Requeued
                | CtxVar::Finished => match ctx.sid.and_then(|sid| self.sessions[sid].req.as_ref())
                {
                    Some(r) => match v {
                        CtxVar::Input => r.input,
                        CtxVar::Output => r.output,
                        CtxVar::Known => r.known,
                        CtxVar::Hit => r.hit,
                        CtxVar::Loaded => r.loaded,
                        CtxVar::Requeued => f64::from(u8::from(r.preempted)),
                        _ => f64::from(u8::from(r.finished)),
                    },
                    None => 0.0,
                },
            },
            CExpr::Agg(a, k) => a.of(self.observed[*k].as_deref().unwrap_or(&[])),
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
                Dist::of(*kind, &a).sample(rng)
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
            _ => unreachable!("Program::validate checks the arguments"),
        };
        let pool = |s: &mut Self, i: usize| match &args[i] {
            CArg::Pool(r) => s.ref_index(r, ctx, w),
            _ => unreachable!("Program::validate checks the arguments"),
        };
        if f.is_arithmetic() {
            // in order: an argument's draw is the stream's next
            let a = num(self, 0);
            let value = if args.len() == 2 {
                f.arithmetic(&[a, num(self, 1)])
            } else {
                f.arithmetic(&[a])
            };
            return value.expect("Program::validate checks the arguments");
        }
        match f {
            Fun::Min
            | Fun::Max
            | Fun::Abs
            | Fun::Floor
            | Fun::Ceil
            | Fun::Sqrt
            | Fun::Exp
            | Fun::Ln => unreachable!("an arithmetic function returned above"),
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
                    // a job in its run's wait is present, not served
                    _ => self.stages[s]
                        .jobs
                        .values()
                        .filter(|j| j.started.is_some())
                        .count() as f64,
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
                        // a job in its run's wait has its work whole, not a tag
                        let vv = v + rate * (now - v_last);
                        st.jobs
                            .values()
                            .map(|j| {
                                if j.started.is_some() {
                                    (j.work - vv).max(0.0)
                                } else {
                                    j.work
                                }
                            })
                            .sum()
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
                        .map(|j| j.started.map_or(j.work, |t| (j.work - (now - t)).max(0.0)))
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
            Fun::Allocated => {
                let p = pool(self, 0);
                match ctx.sid {
                    Some(sid) => self.allocated_by(p, sid),
                    None => 0.0,
                }
            }
            Fun::Holders => {
                let p = pool(self, 0);
                sessions_in(&self.pools[p].holders) as f64
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
            for (serial, e) in pl.cache.live().map(|n| (&n.owner, n.len)) {
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
            .map(|(k, (o, name))| o.report(name, p.observe_is_test(k)))
            .collect();
        let client = self
            .client
            .iter()
            .zip(CLIENT)
            .map(|(o, name)| o.report(name, false))
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
        let statuses = ReqStatus::ALL
            .iter()
            .map(|x| {
                let (stays, avg) = (&self.stays[x.index()], &self.status_avg[x.index()]);
                StatusReport {
                    name: x.name().into(),
                    stays: stays.count(),
                    mean: stays.mean(),
                    p50: stays.quantile(0.5),
                    p99: stays.quantile(0.99),
                    number: avg.mean(now),
                }
            })
            .collect();
        let claims = (0..p.claims.len()).map(|k| self.claim_report(k)).collect();
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
                idle_with_work: s.idle_with_work,
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
        let over = self.heads_over_cap();
        let label = |q: usize| match p.pools[q].index {
            Some(i) => format!("{}[{i}]", p.pools[q].name),
            None => p.pools[q].name.clone(),
        };
        let pools = self
            .pools
            .iter()
            .zip(&p.pools)
            .enumerate()
            .map(|(i, (pl, cp))| PoolReport {
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
                over_cap: over
                    .iter()
                    .find(|(asked, _, _)| *asked == i)
                    .map(|&(_, q, need)| (label(q), need)),
                growing_at_end: self.sessions.iter().filter(|s| grows_in(s, i)).count() as u64,
                growing_stalled: self.sessions.iter().any(|s| grows_in(s, i))
                    && self
                        .sessions
                        .iter()
                        .filter(|s| allocates_in(s, i))
                        .all(|s| grows_in(s, i)),
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
            client,
            gauges,
            statuses,
            claims,
            stages,
            pools,
            log: self.log.take(),
        }
    }

    /// What the run found of claim `k`: an iteration claim from what its
    /// iterations read, a claim `at end` read now, when no session is live.
    fn claim_report(&mut self, k: usize) -> ClaimReport {
        let p = self.p;
        let c = &p.claims[k];
        let state = &self.claims[k];
        let mut r = ClaimReport {
            name: c.name.clone(),
            kind: c.kind,
            result: ClaimResult::Holds,
            checked: state.checked,
            failures: state.failures,
            first: state.first,
            note: None,
        };
        if let Some(serial) = state.out_of_scope {
            r.result = ClaimResult::OutOfScope;
            r.note = Some(format!("session {serial} fails `given`"));
            return r;
        }
        match c.kind {
            ClaimKind::EveryIteration(_) => {
                if r.failures > 0 {
                    r.result = ClaimResult::Fails;
                }
            }
            ClaimKind::SomeIteration(_) => {
                r.result = if r.first.is_some() {
                    ClaimResult::Witnessed
                } else {
                    ClaimResult::NotWitnessed
                };
            }
            ClaimKind::AtEnd => {
                if self.live > 0 {
                    r.result = ClaimResult::NotEvaluated;
                    r.note = Some(format!(
                        "{} session{} live at the end",
                        self.live,
                        if self.live == 1 { "" } else { "s" }
                    ));
                } else {
                    r.checked = 1;
                    if self.eval(&c.expr, &Ctx::default(), Which::Session) == 0.0 {
                        r.result = ClaimResult::Fails;
                        r.failures = 1;
                        r.first = Some(self.now);
                    }
                }
            }
        }
        r
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
        CExpr::Sample(..) | CExpr::Agg(..) => true,
        CExpr::Call(f, _) => {
            !(f.is_arithmetic()
                || matches!(f, Fun::Price | Fun::EstLambda | Fun::EstRho | Fun::EstWait))
        }
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
