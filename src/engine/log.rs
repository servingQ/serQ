//! The log of a run: what each session did, when. An opt-in record for a
//! replay of the run over the deployment view (serq-lar animates it), where
//! the report keeps aggregates. Off by default and costing one branch per
//! event when off (`Interp::logged`); bounded by an event cap when on, since
//! an hour of a busy engine is ~10⁶ iterations.
//!
//! Every event names what the IR names: a session by its serial (a leg of
//! it, `fork`, by its serial and its leg number), a stage and a pool by
//! their index in the program, which the log's header lists with their
//! names. The events are in time order; ties keep the order they happened.

use std::fmt::Write as _;

use crate::engine::report::{json_index, json_num, json_string};
use crate::frontend::ast::RunMode;

/// Version of `Log::json`'s shape, by the rules of `Report::json`'s
/// (`REPORT_VERSION`).
pub const LOG_VERSION: u32 = 0;

/// Who an event is about: a session, or a leg of one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// An iteration of engine `stage` starts, over the requests of `reqs`,
    /// scheduling `tokens`, lasting `dur`.
    Iter {
        stage: usize,
        dur: f64,
        tokens: f64,
        reqs: Vec<Who>,
    },
    /// It ends.
    End { who: Who },
}

/// What the run logged, with the names its indices refer to.
#[derive(Clone, Debug)]
pub struct Log {
    /// Per stage: its name and, in an array, its index.
    pub stages: Vec<(String, Option<u32>)>,
    pub pools: Vec<(String, Option<u32>)>,
    pub events: Vec<Event>,
    /// The most events kept: past it, the rest of the run is not logged.
    pub cap: usize,
    pub truncated: bool,
}

impl Log {
    pub fn new(p: &crate::ir::Program, cap: usize) -> Log {
        Log {
            stages: p.stages.iter().map(|s| (s.name.clone(), s.index)).collect(),
            pools: p.pools.iter().map(|q| (q.name.clone(), q.index)).collect(),
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
        let _ = write!(s, "{{\"version\":{LOG_VERSION},");
        for (key, list) in [("stages", &self.stages), ("pools", &self.pools)] {
            let _ = write!(s, "\"{key}\":[");
            for (i, (name, index)) in list.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                let _ = write!(
                    s,
                    "{{\"name\":{},\"index\":{}}}",
                    json_string(name),
                    json_index(*index)
                );
            }
            s.push_str("],");
        }
        let _ = write!(s, "\"truncated\":{},\"events\":[", self.truncated);
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
                    match mode {
                        RunMode::Plain => {}
                        RunMode::Prefill => s.push_str(",\"mode\":\"prefill\""),
                        RunMode::Decode => s.push_str(",\"mode\":\"decode\""),
                    }
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
                EventKind::Iter {
                    stage,
                    dur,
                    tokens,
                    reqs,
                } => {
                    // each request as `[serial]`, or `[serial, leg]` for a leg
                    let list: Vec<String> = reqs
                        .iter()
                        .map(|w| match w.leg {
                            Some(l) => format!("[{},{l}]", w.serial),
                            None => format!("[{}]", w.serial),
                        })
                        .collect();
                    let _ = write!(
                        s,
                        ",\"ev\":\"iter\",\"stage\":{stage},\"dur\":{},\"tokens\":{},\"reqs\":[{}]",
                        json_num(*dur),
                        json_num(*tokens),
                        list.join(",")
                    );
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cap_truncates_and_says_so() {
        let mut l = Log {
            stages: vec![("E".into(), None)],
            pools: vec![],
            events: vec![],
            cap: 1,
            truncated: false,
        };
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
    fn a_leg_and_a_flow_are_written_out() {
        let mut l = Log {
            stages: vec![("P.nic".into(), Some(0)), ("D.nic".into(), Some(1))],
            pools: vec![("kv".into(), None)],
            events: vec![],
            cap: 10,
            truncated: false,
        };
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
                reqs: vec![
                    w,
                    Who {
                        serial: 8,
                        leg: None,
                    },
                ],
            },
        );
        assert_eq!(
            l.json(),
            r#"{"version":0,"stages":[{"name":"P.nic","index":0},{"name":"D.nic","index":1}],"pools":[{"name":"kv","index":null}],"truncated":false,"events":[{"t":1.25,"ev":"run","s":7,"leg":0,"stages":[0,1]},{"t":1.5,"ev":"iter","stage":1,"dur":0.01,"tokens":16,"reqs":[[7,0],[8]]}]}"#
        );
    }
}
