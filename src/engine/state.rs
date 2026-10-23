//! A request's status: where one call of an engine (`E (input, output)`,
//! vLLM's `Request`) stands with the engine, exactly one at any time, and
//! the KV moves of it in flight (`Transfer`), on an axis of their own
//! (`docs/design/request-state.md`). vLLM keeps the same two apart: a
//! request's `RequestStatus` (`vllm/v1/request.py`, request.py:370) and its
//! connector's sends and receives (`finished_sending`, `finished_recving`,
//! scheduler.py:3126-3138). A program does not read either.

/// Where an engine's request stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    /// In the engine's waiting queue, not scheduled; after a load, with
    /// its blocks, waiting for a running slot. vLLM's `WAITING`.
    Waiting,
    /// Admitted to its blocks, not schedulable until its load is done.
    /// vLLM's `WAITING_FOR_REMOTE_KVS` (scheduler.py:1264-1267).
    WaitingForKv,
    /// A resident of the engine's batch, its mode (prefill, decode) beside
    /// it; a resident stalled for room to grow is one too. vLLM's
    /// `RUNNING`.
    Running,
    /// In the waiting queue again after a preemption, until it runs.
    /// vLLM's `PREEMPTED` (scheduler.py:1560, 3088-3091).
    Preempted,
    /// It has generated its output (or was refused). The report counts it
    /// only while it keeps its blocks: while a save or a send of it is
    /// registered or in flight (vLLM's `delay_free_blocks`,
    /// scheduler.py:2653-2655).
    Finished,
}

impl Status {
    /// Every status, in the report's order.
    pub const ALL: [Status; 5] = [
        Status::Waiting,
        Status::WaitingForKv,
        Status::Running,
        Status::Preempted,
        Status::Finished,
    ];

    /// Its name in the log and the report.
    pub fn name(self) -> &'static str {
        match self {
            Status::Waiting => "waiting",
            Status::WaitingForKv => "waiting_for_kv",
            Status::Running => "running",
            Status::Preempted => "preempted",
            Status::Finished => "finished",
        }
    }

    /// Its place in `ALL`.
    pub fn index(self) -> usize {
        Status::ALL
            .iter()
            .position(|&s| s == self)
            .expect("every status is in ALL")
    }

    /// The status `name` names: the log read back (`Log::from_json`).
    pub fn from_name(name: &str) -> Option<Status> {
        Status::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// A KV move of a request (`ir::CTransfer`, or a source's send).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    /// The KV it reads in at its admission, while `waiting_for_kv`.
    Load,
    /// The KV it writes to an L2 before its blocks go back, `finished`.
    Save,
    /// Its KV kept for a reader's load, `finished`: a KV relation's source.
    Send,
}

impl Transfer {
    /// Every kind of transfer.
    pub const ALL: [Transfer; 3] = [Transfer::Load, Transfer::Save, Transfer::Send];

    /// Its name in the log.
    pub fn name(self) -> &'static str {
        match self {
            Transfer::Load => "load",
            Transfer::Save => "save",
            Transfer::Send => "send",
        }
    }

    /// The transfer `name` names: the log read back (`Log::from_json`).
    pub fn from_name(name: &str) -> Option<Transfer> {
        Transfer::ALL.into_iter().find(|t| t.name() == name)
    }
}

/// How far a transfer has gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moved {
    /// It exists: a load or a save its request has begun, a send its
    /// request's end has left for a reader.
    Registered,
    /// Its copy runs (after any wait for its source, its link or its room).
    Start,
    /// It is over: copied, or a send read or expired.
    Done,
}

impl Moved {
    /// Every step of a transfer, in order.
    pub const ALL: [Moved; 3] = [Moved::Registered, Moved::Start, Moved::Done];

    /// Its name in the log.
    pub fn name(self) -> &'static str {
        match self {
            Moved::Registered => "registered",
            Moved::Start => "start",
            Moved::Done => "done",
        }
    }

    /// The step `name` names: the log read back (`Log::from_json`).
    pub fn from_name(name: &str) -> Option<Moved> {
        Moved::ALL.into_iter().find(|m| m.name() == name)
    }
}
