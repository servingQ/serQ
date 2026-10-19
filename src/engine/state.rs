//! A request's state: where a session, or a leg of one (`fork`), stands
//! while time passes, at about the resolution of vLLM's `RequestStatus`
//! (`vllm/v1/request.py`, request.py:370), for every program and not only
//! for an engine's requests (`docs/design/request-state.md`).
//!
//! The interpreter keeps two things about a session that this is the image
//! of: where its statements stand (a pool's queue, a job at a stage, a
//! `grow` waiting for room, a `join`) and how far its engine request has
//! gone (admitted, its connector loading, prefill, decode, its connector
//! saving). They are not one: a connector's load is a block of statements,
//! which waits in queues and runs on stages of its own. The state is the one
//! function of the two that the run log and the report read
//! (`Interp::state_of`). A program does not read it.
//!
//! A session that is ready, executing statements, is in no state: no time
//! passes then. One that has ended is finished: the log's `end`.

/// What a session or a leg is doing while time passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    /// Queued: in a pool's queue for a hold or an engine's admission, or in
    /// a FIFO stage's queue for a server. vLLM's `WAITING`.
    Waiting,
    /// Queued again after a preemption: the hold that was preempted, or a
    /// request preempted since it was called. vLLM's `PREEMPTED`
    /// (scheduler.py:1560), kept until it runs again (scheduler.py:3088-3091).
    Preempted,
    /// Its engine's connector loads its KV: whatever the connector's `load`
    /// block does, waiting or running. vLLM's `WAITING_FOR_REMOTE_KVS`
    /// (scheduler.py:1267).
    Loading,
    /// A job at a stage: an engine's resident (with its mode, prefill or
    /// decode, as an attribute), a server's job, a `ps` or `delay` stage's.
    /// vLLM's `RUNNING`.
    Running,
    /// A run over shared stages, held at once (`also`, `transfer on`): a
    /// KV transfer between two NICs. No vLLM status: the connector's.
    Transferring,
    /// Its engine's connector saves its KV before its pools go back
    /// (`save`): vLLM's finished request whose blocks the connector keeps
    /// (`delay_free_blocks`, scheduler.py:2653-2655).
    Saving,
    /// A `grow` that did not fit, waiting for room (the pool's `preempt
    /// none`, or no victim). No vLLM status: vLLM preempts instead.
    Growing,
    /// At a `join`, waiting for the legs it forked.
    Joining,
    /// A run the workload issues: the client's tool or think time, between
    /// a turn's answer and the next. No vLLM request exists then.
    Client,
}

impl State {
    /// Every state, in the report's order.
    pub const ALL: [State; 9] = [
        State::Waiting,
        State::Preempted,
        State::Loading,
        State::Running,
        State::Transferring,
        State::Saving,
        State::Growing,
        State::Joining,
        State::Client,
    ];

    /// Its name in the log and the report.
    pub fn name(self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::Preempted => "preempted",
            State::Loading => "loading",
            State::Running => "running",
            State::Transferring => "transferring",
            State::Saving => "saving",
            State::Growing => "growing",
            State::Joining => "joining",
            State::Client => "client",
        }
    }

    /// Its place in `ALL`.
    pub fn index(self) -> usize {
        State::ALL
            .iter()
            .position(|&s| s == self)
            .expect("every state is in ALL")
    }
}
