//! The log of a run: what each session did, when. An opt-in record for a
//! replay of the run over the deployment view (serq-lar animates it), where
//! the report keeps aggregates. Off by default and costing one branch per
//! event when off (`Interp::logged`); bounded by an event cap when on, since
//! an hour of a busy engine is ~10⁶ iterations.
//!
//! Every event names what the IR names: a session by its serial (a leg of
//! it, `fork`, by its serial and its leg number), a stage and a pool by
//! their index in the program, which the log's header lists with their
//! names; an iteration's `used`, `cached` and `evicted` are its engine's
//! memory pool's, which the IR names (`CStep::memory`) and the header gives
//! with the pool's `cap`. The events are in time order; ties keep the order
//! they happened.
//!
//! `Log::json` writes it and `Log::from_json` reads it back, for a figure
//! drawn from a run that already ran (`view::iterations`).

use std::borrow::Cow;
use std::fmt::Write as _;

use serde::Deserialize;
use serde::de::{Deserializer, Error as _, SeqAccess, Visitor};

use crate::engine::report::{json_index, json_num, json_string, label};
use crate::engine::state::{Moved, Status, Transfer};
use crate::frontend::ast::RunMode;
use crate::ir::CStageKind;

/// Version of `Log::json`'s shape, by the rules of `Report::json`'s
/// (`REPORT_VERSION`).
pub const LOG_VERSION: u32 = 0;

/// Who an event is about: a session, or a leg of one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Who {
    pub serial: u64,
    /// The leg's number among the session's forks; `None` for the session.
    pub leg: Option<u32>,
}

/// One thing that happened, at `t`.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub t: f64,
    pub kind: EventKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EventKind {
    /// A session arrives.
    Arrive { who: Who },
    /// A leg of a session is forked (`who.leg` is its number).
    Fork { who: Who },
    /// A turn of the session starts: its `turn` number.
    Turn { who: Who, turn: u32 },
    /// It waits in the queue of `pool` for a hold (of several pools: the
    /// queue of its first).
    Wait { who: Who, pool: usize },
    /// It is admitted to the hold it waited for at `pool`.
    Admit { who: Who, pool: usize },
    /// A hold that can never fit refuses it: it ends.
    Refuse { who: Who },
    /// It runs on `stages` (several: a flow holding them at once, a KV
    /// transfer over two NICs), as a prefill or decode on an engine.
    Run {
        who: Who,
        stages: Vec<usize>,
        mode: RunMode,
    },
    /// Its run on `stage` (the first of a flow's) is done.
    Done { who: Who, stage: usize },
    /// It is preempted from `pool`: back to the queue.
    Preempt { who: Who, pool: usize },
    /// Its lease of `pool` is revoked (`preempt leases by`).
    Revoke { who: Who, pool: usize },
    /// Its `call`-th engine request, to engine `stage`, enters `status`
    /// (`engine::state`), or its mode there changes (prefill, decode).
    Status {
        who: Who,
        call: u32,
        stage: usize,
        status: Status,
        mode: RunMode,
    },
    /// A transfer of that request is registered, starts its copy, or is
    /// done. A send's is about the source's request, whoever holds it.
    Transfer {
        who: Who,
        call: u32,
        stage: usize,
        kind: Transfer,
        moved: Moved,
    },
    /// An iteration of engine `stage` starts, over the requests of `reqs`,
    /// scheduling `tokens`, lasting `dur`.
    Iter {
        stage: usize,
        dur: f64,
        tokens: f64,
        /// The engine's `tokens cap` as this iteration read it, infinite
        /// when it is `inf`.
        budget: f64,
        reqs: Vec<Who>,
        /// The tokens of each of `reqs`, in its order: a decode's 1, a
        /// prefill's chunk. They sum to `tokens`.
        each: Vec<f64>,
        /// The engine's memory pool (`CStep::memory`) once the iteration is
        /// scheduled; `None` for an engine without one.
        memory: Option<Memory>,
    },
    /// It ends.
    End { who: Who },
}

/// An engine's memory pool as an iteration finds it, in units (a pool
/// with `block` counts its blocks' units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Memory {
    /// Allocated (`used(p)`): what the report's `mean_used` averages.
    pub used: f64,
    /// Cached: what the report's `mean_cached` averages.
    pub cached: f64,
    /// Evicted from the cache since the run began: the report's
    /// `evicted_units` so far, which a warm-up does not reset. A rise
    /// between two iterations of the engine is an eviction between them.
    pub evicted: f64,
}

/// A stage as the log's header names it.
#[derive(Clone, Debug, PartialEq)]
pub struct LogStage {
    pub name: String,
    /// Its index in an array (`stage E[N]`), `None` for a single stage.
    pub index: Option<u32>,
    /// An engine's memory pool (`CStep::memory`), whose use its
    /// iterations give; `None` for a stage that is no engine or an engine
    /// without one.
    pub memory: Option<usize>,
}

/// A pool as the log's header names it.
#[derive(Clone, Debug, PartialEq)]
pub struct LogPool {
    pub name: String,
    pub index: Option<u32>,
    /// Its capacity in units, infinite without a `cap`: what an engine's
    /// `used` and `cached` together never pass.
    pub cap: f64,
}

impl LogStage {
    /// `E`, or `E[0]` for an array's member.
    pub fn label(&self) -> String {
        label(&self.name, self.index)
    }
}

impl LogPool {
    /// `kv`, or `kv[0]` for an array's member.
    pub fn label(&self) -> String {
        label(&self.name, self.index)
    }
}

/// What the run logged, with the names its indices refer to.
#[derive(Clone, Debug)]
pub struct Log {
    pub stages: Vec<LogStage>,
    pub pools: Vec<LogPool>,
    pub events: Vec<Event>,
    /// The most events kept: past it, the rest of the run is not logged.
    pub cap: usize,
    pub truncated: bool,
}

impl Log {
    pub fn new(p: &crate::ir::Program, cap: usize) -> Log {
        Log {
            stages: p
                .stages
                .iter()
                .map(|s| LogStage {
                    name: s.name.clone(),
                    index: s.index,
                    memory: match &s.kind {
                        CStageKind::Step(spec) => spec.memory,
                        _ => None,
                    },
                })
                .collect(),
            pools: p
                .pools
                .iter()
                .map(|q| LogPool {
                    name: q.name.clone(),
                    index: q.index,
                    cap: q.cap,
                })
                .collect(),
            events: vec![],
            cap,
            truncated: false,
        }
    }

    pub fn push(&mut self, t: f64, kind: EventKind) {
        if self.events.len() >= self.cap {
            self.truncated = true;
            return;
        }
        self.events.push(Event { t, kind });
    }

    /// The log as JSON: `{"version", "stages", "pools", "truncated",
    /// "events"}`, each event `{"t", "ev", …}` with `s` (and `leg`) for who
    /// it is about. The keys are short because the events are many: an
    /// hour of an engine is ~10⁶ iterations, each with its batch.
    pub fn json(&self) -> String {
        let mut s = String::new();
        let _ = write!(s, "{{\"version\":{LOG_VERSION},\"stages\":[");
        for (i, st) in self.stages.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(
                s,
                "{{\"name\":{},\"index\":{}",
                json_string(&st.name),
                json_index(st.index)
            );
            // an engine's memory pool, absent for a stage without one
            if let Some(m) = st.memory {
                let _ = write!(s, ",\"memory\":{m}");
            }
            s.push('}');
        }
        s.push_str("],\"pools\":[");
        for (i, q) in self.pools.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(
                s,
                "{{\"name\":{},\"index\":{},\"cap\":{}}}",
                json_string(&q.name),
                json_index(q.index),
                json_num(q.cap)
            );
        }
        let _ = write!(s, "],\"truncated\":{},\"events\":[", self.truncated);
        for (i, e) in self.events.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(s, "{{\"t\":{}", json_num(e.t));
            let who = |s: &mut String, w: &Who| {
                let _ = write!(s, ",\"s\":{}", w.serial);
                if let Some(l) = w.leg {
                    let _ = write!(s, ",\"leg\":{l}");
                }
            };
            match &e.kind {
                EventKind::Arrive { who: w } => {
                    s.push_str(",\"ev\":\"arrive\"");
                    who(&mut s, w);
                }
                EventKind::Fork { who: w } => {
                    s.push_str(",\"ev\":\"fork\"");
                    who(&mut s, w);
                }
                EventKind::Turn { who: w, turn } => {
                    s.push_str(",\"ev\":\"turn\"");
                    who(&mut s, w);
                    let _ = write!(s, ",\"turn\":{turn}");
                }
                EventKind::Wait { who: w, pool } => {
                    s.push_str(",\"ev\":\"wait\"");
                    who(&mut s, w);
                    let _ = write!(s, ",\"pool\":{pool}");
                }
                EventKind::Admit { who: w, pool } => {
                    s.push_str(",\"ev\":\"admit\"");
                    who(&mut s, w);
                    let _ = write!(s, ",\"pool\":{pool}");
                }
                EventKind::Refuse { who: w } => {
                    s.push_str(",\"ev\":\"refuse\"");
                    who(&mut s, w);
                }
                EventKind::Run {
                    who: w,
                    stages,
                    mode,
                } => {
                    s.push_str(",\"ev\":\"run\"");
                    who(&mut s, w);
                    let list: Vec<String> = stages.iter().map(|k| k.to_string()).collect();
                    let _ = write!(s, ",\"stages\":[{}]", list.join(","));
                    write_mode(&mut s, *mode);
                }
                EventKind::Done { who: w, stage } => {
                    s.push_str(",\"ev\":\"done\"");
                    who(&mut s, w);
                    let _ = write!(s, ",\"stage\":{stage}");
                }
                EventKind::Preempt { who: w, pool } => {
                    s.push_str(",\"ev\":\"preempt\"");
                    who(&mut s, w);
                    let _ = write!(s, ",\"pool\":{pool}");
                }
                EventKind::Revoke { who: w, pool } => {
                    s.push_str(",\"ev\":\"revoke\"");
                    who(&mut s, w);
                    let _ = write!(s, ",\"pool\":{pool}");
                }
                EventKind::Status {
                    who: w,
                    call,
                    stage,
                    status,
                    mode,
                } => {
                    s.push_str(",\"ev\":\"status\"");
                    who(&mut s, w);
                    let _ = write!(
                        s,
                        ",\"r\":{call},\"stage\":{stage},\"status\":\"{}\"",
                        status.name()
                    );
                    write_mode(&mut s, *mode);
                }
                EventKind::Transfer {
                    who: w,
                    call,
                    stage,
                    kind,
                    moved,
                } => {
                    s.push_str(",\"ev\":\"transfer\"");
                    who(&mut s, w);
                    let _ = write!(
                        s,
                        ",\"r\":{call},\"stage\":{stage},\"kind\":\"{}\",\"at\":\"{}\"",
                        kind.name(),
                        moved.name()
                    );
                }
                EventKind::Iter {
                    stage,
                    dur,
                    tokens,
                    budget,
                    reqs,
                    each,
                    memory,
                } => {
                    // each request as `[serial]`, or `[serial, leg]` for a leg
                    let list: Vec<String> = reqs
                        .iter()
                        .map(|w| match w.leg {
                            Some(l) => format!("[{},{l}]", w.serial),
                            None => format!("[{}]", w.serial),
                        })
                        .collect();
                    let each: Vec<String> = each.iter().map(|&n| json_num(n)).collect();
                    let _ = write!(
                        s,
                        ",\"ev\":\"iter\",\"stage\":{stage},\"dur\":{},\"tokens\":{},\"budget\":{},\"reqs\":[{}],\"each\":[{}]",
                        json_num(*dur),
                        json_num(*tokens),
                        json_num(*budget),
                        list.join(","),
                        each.join(",")
                    );
                    // the memory pool's, absent for an engine without one
                    if let Some(m) = memory {
                        let _ = write!(
                            s,
                            ",\"used\":{},\"cached\":{},\"evicted\":{}",
                            json_num(m.used),
                            json_num(m.cached),
                            json_num(m.evicted)
                        );
                    }
                }
                EventKind::End { who: w } => {
                    s.push_str(",\"ev\":\"end\"");
                    who(&mut s, w);
                }
            }
            s.push('}');
        }
        s.push_str("]}");
        s
    }

    /// The log `json` wrote, read back. A log of another `version` is
    /// refused, and so is one without what this serq writes and a reader
    /// needs (a pool's `cap`, an iteration's `budget` and `each`): an older
    /// serq's, which is to be run again. An event this serq does not know
    /// is passed over, as an added event may be (`docs/reference/log.md`).
    /// `cap` is the number of events read.
    pub fn from_json(text: &str) -> Result<Log, String> {
        let raw: RawLog = match serde_json::from_str(text) {
            Ok(raw) => raw,
            // another version's shape may not parse: say which it is; an
            // IR has a `version` too, but no `events`
            Err(e) => {
                return Err(match serde_json::from_str::<Version>(text) {
                    Ok(v) if v.version != LOG_VERSION => another_version(v.version),
                    Ok(_) => e.to_string(),
                    Err(_) => format!("not a run log: {e}"),
                });
            }
        };
        if raw.version != LOG_VERSION {
            return Err(another_version(raw.version));
        }
        let stages: Vec<LogStage> = raw
            .stages
            .into_iter()
            .map(|s| LogStage {
                name: s.name,
                index: s.index,
                memory: s.memory,
            })
            .collect();
        let pools = raw
            .pools
            .into_iter()
            .map(|q| match q.cap {
                Some(cap) => Ok(LogPool {
                    name: q.name,
                    index: q.index,
                    cap: cap.unwrap_or(f64::INFINITY),
                }),
                None => Err(older(&format!("pool `{}` has no `cap`", q.name))),
            })
            .collect::<Result<Vec<LogPool>, String>>()?;
        let log = Log {
            cap: raw.events.0.len(),
            stages,
            pools,
            events: raw.events.0,
            truncated: raw.truncated,
        };
        log.check_indices()?;
        Ok(log)
    }

    /// Every stage and pool an event or the header names is one of the
    /// header's: a reader indexes them.
    fn check_indices(&self) -> Result<(), String> {
        let (ns, np) = (self.stages.len(), self.pools.len());
        let bad = |what: &str, k: usize, n: usize| {
            Err(format!(
                "{what} {k} is not in the log's header, which has {n}"
            ))
        };
        for s in &self.stages {
            if let Some(m) = s.memory.filter(|&m| m >= np) {
                return bad("the memory pool", m, np);
            }
        }
        for e in &self.events {
            let (stages, pools): (&[usize], &[usize]) = match &e.kind {
                EventKind::Wait { pool, .. }
                | EventKind::Admit { pool, .. }
                | EventKind::Preempt { pool, .. }
                | EventKind::Revoke { pool, .. } => (&[], std::slice::from_ref(pool)),
                EventKind::Run { stages, .. } => (stages, &[]),
                EventKind::Done { stage, .. }
                | EventKind::Status { stage, .. }
                | EventKind::Transfer { stage, .. }
                | EventKind::Iter { stage, .. } => (std::slice::from_ref(stage), &[]),
                EventKind::Arrive { .. }
                | EventKind::Fork { .. }
                | EventKind::Turn { .. }
                | EventKind::Refuse { .. }
                | EventKind::End { .. } => (&[], &[]),
            };
            if let Some(&k) = stages.iter().find(|&&k| k >= ns) {
                return bad("stage", k, ns);
            }
            if let Some(&k) = pools.iter().find(|&&k| k >= np) {
                return bad("pool", k, np);
            }
        }
        Ok(())
    }
}

/// A log of version `v`, which this serq does not read.
fn another_version(v: u32) -> String {
    format!("a run log of version {v}; this serq reads version {LOG_VERSION}")
}

/// A log without what this serq writes: an older serq's.
fn older(what: &str) -> String {
    format!(
        "{what}: the log was written by an older serq; run the program again with this one (serq {}) to log it",
        crate::VERSION
    )
}

/// The engine's modes as the log names them, `"mode"` in a `run` or a
/// `status` event; `Plain` has none.
const MODES: [RunMode; 3] = [RunMode::Plain, RunMode::Prefill, RunMode::Decode];

/// The name the log gives `mode`; the reader finds it again through `MODES`.
fn mode_name(mode: RunMode) -> Option<&'static str> {
    match mode {
        RunMode::Plain => None,
        RunMode::Prefill => Some("prefill"),
        RunMode::Decode => Some("decode"),
    }
}

/// An event's `"mode"` field, or nothing for `Plain`.
fn write_mode(s: &mut String, mode: RunMode) {
    if let Some(name) = mode_name(mode) {
        let _ = write!(s, ",\"mode\":\"{name}\"");
    }
}

/// Just the version of a log, to say so when the rest does not parse.
#[derive(Deserialize)]
struct Version {
    version: u32,
    #[allow(dead_code)]
    events: serde::de::IgnoredAny,
}

#[derive(Deserialize)]
struct RawLog {
    version: u32,
    stages: Vec<RawStage>,
    pools: Vec<RawPool>,
    truncated: bool,
    events: Events,
}

#[derive(Deserialize)]
struct RawStage {
    name: String,
    index: Option<u32>,
    #[serde(default)]
    memory: Option<usize>,
}

#[derive(Deserialize)]
struct RawPool {
    name: String,
    index: Option<u32>,
    /// Absent (`None`) in an older log; `null` (`Some(None)`) is `inf`.
    #[serde(default, deserialize_with = "present")]
    cap: Option<Option<f64>>,
}

/// A key that is there, `null` or not: `Some`, where an absent one is the
/// field's default, `None`.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

/// One event as written: every key any event has, each optional.
#[derive(Deserialize)]
struct RawEvent<'a> {
    t: f64,
    #[serde(borrow)]
    ev: Cow<'a, str>,
    s: Option<u64>,
    leg: Option<u32>,
    turn: Option<u32>,
    pool: Option<usize>,
    stages: Option<Vec<usize>>,
    #[serde(borrow)]
    mode: Option<Cow<'a, str>>,
    stage: Option<usize>,
    r: Option<u32>,
    #[serde(borrow)]
    status: Option<Cow<'a, str>>,
    #[serde(borrow)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow)]
    at: Option<Cow<'a, str>>,
    dur: Option<f64>,
    tokens: Option<f64>,
    #[serde(default, deserialize_with = "present")]
    budget: Option<Option<f64>>,
    reqs: Option<Vec<Vec<u64>>>,
    each: Option<Vec<f64>>,
    used: Option<f64>,
    cached: Option<f64>,
    evicted: Option<f64>,
}

impl RawEvent<'_> {
    /// The event, or `None` for an `ev` this serq does not know.
    fn event(self) -> Result<Option<Event>, String> {
        let RawEvent {
            t,
            ev,
            s,
            leg,
            turn,
            pool,
            stages,
            mode,
            stage,
            r,
            status,
            kind,
            at,
            dur,
            tokens,
            budget,
            reqs,
            each,
            used,
            cached,
            evicted,
        } = self;
        let need = |key: &str| format!("an `{ev}` event at t = {t} has no `{key}`");
        let who = || -> Result<Who, String> {
            Ok(Who {
                serial: s.ok_or_else(|| need("s"))?,
                leg,
            })
        };
        let pool = || pool.ok_or_else(|| need("pool"));
        let stage = || stage.ok_or_else(|| need("stage"));
        let call = || r.ok_or_else(|| need("r"));
        let mode = || match mode.as_deref() {
            None => Ok(RunMode::Plain),
            Some(m) => MODES
                .into_iter()
                .find(|&k| mode_name(k) == Some(m))
                .ok_or_else(|| format!("unknown mode `{m}` at t = {t}")),
        };
        // a name of `table`'s, read back by `from_name`
        fn named<T>(
            name: Option<&str>,
            key: &str,
            from_name: fn(&str) -> Option<T>,
            need: impl Fn(&str) -> String,
        ) -> Result<T, String> {
            let name = name.ok_or_else(|| need(key))?;
            from_name(name).ok_or_else(|| format!("unknown {key} `{name}`"))
        }
        let kind = match &*ev {
            "arrive" => EventKind::Arrive { who: who()? },
            "fork" => EventKind::Fork { who: who()? },
            "turn" => EventKind::Turn {
                who: who()?,
                turn: turn.ok_or_else(|| need("turn"))?,
            },
            "wait" => EventKind::Wait {
                who: who()?,
                pool: pool()?,
            },
            "admit" => EventKind::Admit {
                who: who()?,
                pool: pool()?,
            },
            "refuse" => EventKind::Refuse { who: who()? },
            "run" => EventKind::Run {
                who: who()?,
                stages: stages.ok_or_else(|| need("stages"))?,
                mode: mode()?,
            },
            "done" => EventKind::Done {
                who: who()?,
                stage: stage()?,
            },
            "preempt" => EventKind::Preempt {
                who: who()?,
                pool: pool()?,
            },
            "revoke" => EventKind::Revoke {
                who: who()?,
                pool: pool()?,
            },
            "status" => EventKind::Status {
                who: who()?,
                call: call()?,
                stage: stage()?,
                status: named(status.as_deref(), "status", Status::from_name, need)?,
                mode: mode()?,
            },
            "transfer" => EventKind::Transfer {
                who: who()?,
                call: call()?,
                stage: stage()?,
                kind: named(kind.as_deref(), "kind", Transfer::from_name, need)?,
                moved: named(at.as_deref(), "at", Moved::from_name, need)?,
            },
            "iter" => {
                let (Some(budget), Some(each)) = (budget, each) else {
                    return Err(older(&format!(
                        "the iteration at t = {t} has no `budget` or `each`"
                    )));
                };
                let reqs = reqs
                    .ok_or_else(|| need("reqs"))?
                    .into_iter()
                    .map(|q| match q[..] {
                        [serial] => Ok(Who { serial, leg: None }),
                        [serial, leg] if u32::try_from(leg).is_ok() => Ok(Who {
                            serial,
                            leg: Some(leg as u32),
                        }),
                        _ => Err(format!(
                            "a request of the iteration at t = {t} is {q:?}, not [s] or [s, leg]"
                        )),
                    })
                    .collect::<Result<Vec<Who>, String>>()?;
                if each.len() != reqs.len() {
                    return Err(format!(
                        "the iteration at t = {t} has {} requests and {} counts in `each`",
                        reqs.len(),
                        each.len()
                    ));
                }
                let memory = match (used, cached, evicted) {
                    (None, None, None) => None,
                    (Some(used), Some(cached), Some(evicted)) => Some(Memory {
                        used,
                        cached,
                        evicted,
                    }),
                    (Some(_), Some(_), None) => {
                        return Err(older(&format!("the iteration at t = {t} has no `evicted`")));
                    }
                    _ => return Err(need("used`, `cached` or `evicted")),
                };
                EventKind::Iter {
                    stage: stage()?,
                    dur: dur.ok_or_else(|| need("dur"))?,
                    tokens: tokens.ok_or_else(|| need("tokens"))?,
                    budget: budget.unwrap_or(f64::INFINITY),
                    reqs,
                    each,
                    memory,
                }
            }
            "end" => EventKind::End { who: who()? },
            _ => return Ok(None),
        };
        Ok(Some(Event { t, kind }))
    }
}

/// The events, each converted as it is read: the log of a long run is
/// large (an hour of a busy engine is ~10⁶ iterations), and only the
/// events are kept, not their text.
struct Events(Vec<Event>);

impl<'de> Deserialize<'de> for Events {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Events, D::Error> {
        struct Seq;
        impl<'de> Visitor<'de> for Seq {
            type Value = Events;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a list of events")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Events, A::Error> {
                let mut events = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(raw) = seq.next_element::<RawEvent>()? {
                    if let Some(e) = raw.event().map_err(A::Error::custom)? {
                        events.push(e);
                    }
                }
                Ok(Events(events))
            }
        }
        d.deserialize_seq(Seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A log of `stages` (each with its memory pool) and `pools` (each
    /// with its cap), keeping at most `cap` events.
    fn log(
        stages: &[(&str, Option<u32>, Option<usize>)],
        pools: &[(&str, f64)],
        cap: usize,
    ) -> Log {
        Log {
            stages: stages
                .iter()
                .map(|&(name, index, memory)| LogStage {
                    name: name.into(),
                    index,
                    memory,
                })
                .collect(),
            pools: pools
                .iter()
                .map(|&(name, cap)| LogPool {
                    name: name.into(),
                    index: None,
                    cap,
                })
                .collect(),
            events: vec![],
            cap,
            truncated: false,
        }
    }

    #[test]
    fn the_cap_truncates_and_says_so() {
        let mut l = log(&[("E", None, None)], &[], 1);
        let w = Who {
            serial: 3,
            leg: None,
        };
        l.push(0.0, EventKind::Arrive { who: w });
        l.push(0.5, EventKind::End { who: w });
        assert_eq!(l.events.len(), 1);
        assert!(l.truncated);
        assert_eq!(
            l.json(),
            r#"{"version":0,"stages":[{"name":"E","index":null}],"pools":[],"truncated":true,"events":[{"t":0,"ev":"arrive","s":3}]}"#
        );
    }

    #[test]
    fn a_status_and_a_transfer_are_written_out() {
        let mut l = log(&[("E", None, None)], &[], 10);
        let w = Who {
            serial: 2,
            leg: Some(1),
        };
        l.push(
            0.5,
            EventKind::Status {
                who: w,
                call: 3,
                stage: 0,
                status: Status::Running,
                mode: RunMode::Decode,
            },
        );
        l.push(
            1.0,
            EventKind::Status {
                who: w,
                call: 3,
                stage: 0,
                status: Status::Finished,
                mode: RunMode::Plain,
            },
        );
        l.push(
            1.0,
            EventKind::Transfer {
                who: w,
                call: 3,
                stage: 0,
                kind: Transfer::Send,
                moved: Moved::Registered,
            },
        );
        assert_eq!(
            l.json(),
            r#"{"version":0,"stages":[{"name":"E","index":null}],"pools":[],"truncated":false,"events":[{"t":0.5,"ev":"status","s":2,"leg":1,"r":3,"stage":0,"status":"running","mode":"decode"},{"t":1,"ev":"status","s":2,"leg":1,"r":3,"stage":0,"status":"finished"},{"t":1,"ev":"transfer","s":2,"leg":1,"r":3,"stage":0,"kind":"send","at":"registered"}]}"#
        );
    }

    /// The header names an engine's memory pool and a pool's cap, so that
    /// an iteration's `used` and `cached` are read against the cap of the
    /// same run.
    #[test]
    fn a_leg_and_a_flow_are_written_out() {
        let mut l = log(
            &[("P.nic", Some(0), None), ("D", Some(1), Some(0))],
            &[("kv", 64.0)],
            10,
        );
        let w = Who {
            serial: 7,
            leg: Some(0),
        };
        l.push(
            1.25,
            EventKind::Run {
                who: w,
                stages: vec![0, 1],
                mode: RunMode::Plain,
            },
        );
        l.push(
            1.5,
            EventKind::Iter {
                stage: 1,
                dur: 0.01,
                tokens: 16.0,
                budget: 16.0,
                reqs: vec![
                    w,
                    Who {
                        serial: 8,
                        leg: None,
                    },
                ],
                each: vec![10.0, 6.0],
                memory: Some(Memory {
                    used: 32.0,
                    cached: 0.0,
                    evicted: 16.0,
                }),
            },
        );
        assert_eq!(
            l.json(),
            r#"{"version":0,"stages":[{"name":"P.nic","index":0},{"name":"D","index":1,"memory":0}],"pools":[{"name":"kv","index":null,"cap":64}],"truncated":false,"events":[{"t":1.25,"ev":"run","s":7,"leg":0,"stages":[0,1]},{"t":1.5,"ev":"iter","stage":1,"dur":0.01,"tokens":16,"budget":16,"reqs":[[7,0],[8]],"each":[10,6],"used":32,"cached":0,"evicted":16}]}"#
        );
    }

    /// An engine whose `tokens cap` is `inf` has no budget to write but
    /// `null` (the report's convention for a value that is not finite), and
    /// one without a memory pool writes no `used` or `cached`: not 0, which
    /// would claim an empty pool. A pool without a cap has a `null` one.
    #[test]
    fn an_iteration_with_an_infinite_cap_and_no_memory() {
        let mut l = log(&[("E", None, None)], &[("slots", f64::INFINITY)], 10);
        l.push(
            2.0,
            EventKind::Iter {
                stage: 0,
                dur: 0.5,
                tokens: 1.0,
                budget: f64::INFINITY,
                reqs: vec![Who {
                    serial: 4,
                    leg: None,
                }],
                each: vec![1.0],
                memory: None,
            },
        );
        assert_eq!(
            l.json(),
            r#"{"version":0,"stages":[{"name":"E","index":null}],"pools":[{"name":"slots","index":null,"cap":null}],"truncated":false,"events":[{"t":2,"ev":"iter","stage":0,"dur":0.5,"tokens":1,"budget":null,"reqs":[[4]],"each":[1]}]}"#
        );
    }

    /// Every kind of event, each status, transfer and step, and every mode,
    /// written and read back: the reader and the writer name them from the
    /// same tables (`Status::ALL`, `Transfer::ALL`, `Moved::ALL`, `MODES`).
    #[test]
    fn a_log_reads_back_as_written() {
        let mut l = log(
            &[
                ("E", Some(0), Some(1)),
                ("E", Some(1), None),
                ("nic", None, None),
            ],
            &[("reqs", f64::INFINITY), ("kv", 4096.0)],
            usize::MAX,
        );
        l.truncated = true;
        let s = Who {
            serial: 5,
            leg: None,
        };
        let leg = Who {
            serial: 5,
            leg: Some(2),
        };
        let mut t = 0.0;
        let mut at = |kind: EventKind| {
            t += 0.125;
            (t, kind)
        };
        let mut events = vec![
            at(EventKind::Arrive { who: s }),
            at(EventKind::Turn { who: s, turn: 1 }),
            at(EventKind::Fork { who: leg }),
            at(EventKind::Wait { who: leg, pool: 0 }),
            at(EventKind::Admit { who: leg, pool: 1 }),
            at(EventKind::Refuse { who: s }),
            at(EventKind::Done { who: leg, stage: 2 }),
            at(EventKind::Preempt { who: leg, pool: 1 }),
            at(EventKind::Revoke { who: s, pool: 0 }),
            at(EventKind::Iter {
                stage: 0,
                dur: 0.1 + 0.2,
                tokens: 2049.0,
                budget: 4096.0,
                reqs: vec![s, leg],
                each: vec![1.0, 2048.0],
                memory: Some(Memory {
                    used: 1234.5,
                    cached: 0.0,
                    evicted: 1e9,
                }),
            }),
            at(EventKind::Iter {
                stage: 1,
                dur: 1e-7,
                tokens: 1.0,
                budget: f64::INFINITY,
                reqs: vec![leg],
                each: vec![1.0],
                memory: None,
            }),
            at(EventKind::End { who: leg }),
        ];
        for mode in MODES {
            events.push(at(EventKind::Run {
                who: s,
                stages: vec![0, 2],
                mode,
            }));
            for status in Status::ALL {
                events.push(at(EventKind::Status {
                    who: leg,
                    call: 3,
                    stage: 1,
                    status,
                    mode,
                }));
            }
        }
        for kind in Transfer::ALL {
            for moved in Moved::ALL {
                events.push(at(EventKind::Transfer {
                    who: s,
                    call: 1,
                    stage: 0,
                    kind,
                    moved,
                }));
            }
        }
        for (t, kind) in events {
            l.push(t, kind);
        }
        let back = Log::from_json(&l.json()).unwrap();
        assert_eq!(back.stages, l.stages);
        assert_eq!(back.pools, l.pools);
        assert_eq!(back.events, l.events);
        assert_eq!(back.truncated, l.truncated);
        assert_eq!(back.cap, l.events.len());
    }

    /// Another version is refused by its number, whether or not its shape
    /// parses; an IR, which has a `version` too, is no log.
    #[test]
    fn another_version_is_refused() {
        let e =
            Log::from_json(r#"{"version":1,"stages":[],"pools":[],"truncated":false,"events":[]}"#)
                .unwrap_err();
        assert!(e.contains("version 1") && e.contains("version 0"), "{e}");
        let e = Log::from_json(r#"{"version":1,"events":[{"ev":"arrive"}]}"#).unwrap_err();
        assert!(e.contains("version 1") && e.contains("version 0"), "{e}");
        let e = Log::from_json(r#"{"version":15,"pools":[],"stages":[]}"#).unwrap_err();
        assert!(e.starts_with("not a run log"), "{e}");
    }

    /// An event a later serq adds is passed over, as an added field is.
    #[test]
    fn an_unknown_event_is_passed_over() {
        let l = Log::from_json(
            r#"{"version":0,"stages":[],"pools":[],"truncated":false,"events":[{"t":0,"ev":"arrive","s":1,"new":2},{"t":1,"ev":"teleport","s":1},{"t":2,"ev":"end","s":1}]}"#,
        )
        .unwrap();
        assert_eq!(l.events.len(), 2);
        assert!(matches!(l.events[1].kind, EventKind::End { .. }));
    }

    /// A log an older serq wrote (before `each`, `evicted` or a pool's
    /// `cap`) is refused with what to do, not read as if it had them.
    #[test]
    fn an_older_log_is_refused() {
        for text in [
            r#"{"version":0,"stages":[{"name":"E","index":null}],"pools":[],"truncated":false,"events":[{"t":0,"ev":"iter","stage":0,"dur":1,"tokens":1,"reqs":[[0]]}]}"#,
            r#"{"version":0,"stages":[{"name":"E","index":null,"memory":0}],"pools":[{"name":"kv","index":null,"cap":8}],"truncated":false,"events":[{"t":0,"ev":"iter","stage":0,"dur":1,"tokens":1,"budget":4,"reqs":[[0]],"each":[1],"used":1,"cached":0}]}"#,
            r#"{"version":0,"stages":[],"pools":[{"name":"kv","index":null}],"truncated":false,"events":[]}"#,
        ] {
            let e = Log::from_json(text).unwrap_err();
            assert!(
                e.contains("older serq") && e.contains("run the program again"),
                "{e}"
            );
        }
    }

    /// A stage or a pool the header does not have is an error, not a
    /// panic in a reader that indexes the header.
    #[test]
    fn an_index_past_the_header_is_refused() {
        let e = Log::from_json(
            r#"{"version":0,"stages":[{"name":"E","index":null}],"pools":[],"truncated":false,"events":[{"t":0,"ev":"done","s":1,"stage":3}]}"#,
        )
        .unwrap_err();
        assert!(e.contains("stage 3"), "{e}");
    }
}
