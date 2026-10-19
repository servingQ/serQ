# A request's state

A session, or a leg of one (`fork`), is in one state while time passes, at
about the resolution of vLLM's `RequestStatus`: waiting, preempted, loading
its KV, running, and so on. The interpreter keeps it, the run log records
every change of it, and the report gives the time spent in each. It covers
every program, not only an engine's requests: a plain stage, a transfer
over two NICs, a `join`, the client's think time. A program does not read
it: no syntax, no IR change.

## In vLLM

vLLM keeps one status per request (`vllm/v1/request.py`, request.py:370-386):

```python
class RequestStatus(enum.IntEnum):
    WAITING = enum.auto()
    WAITING_FOR_STRUCTURED_OUTPUT_GRAMMAR = enum.auto()
    WAITING_FOR_REMOTE_KVS = enum.auto()
    WAITING_FOR_STREAMING_REQ = enum.auto()
    RUNNING = enum.auto()
    PREEMPTED = enum.auto()
    # Note: anything after PREEMPTED will be considered
    # as a finished status.
    FINISHED_STOPPED = enum.auto()
    FINISHED_LENGTH_CAPPED = enum.auto()
    FINISHED_ABORTED = enum.auto()
    FINISHED_IGNORED = enum.auto()
    FINISHED_ERROR = enum.auto()
    FINISHED_REPETITION = enum.auto()
```

A request is created `WAITING` (request.py:100). The scheduler admits it
and sets `RUNNING` whether it was `WAITING` or `PREEMPTED`
(scheduler.py:1304-1319). A request whose connector loads its KV
asynchronously is given its blocks and set `WAITING_FOR_REMOTE_KVS`
(scheduler.py:1264-1267); once the KV has arrived it is `PREEMPTED` again
if it has ever been preempted, `WAITING` otherwise, and waits to be
scheduled like any other (scheduler.py:3081-3091). A preemption frees the
request's blocks and sets `PREEMPTED`, keeping its tokens
(`_preempt_request`, scheduler.py:1560-1561). Prefill and decode are not
statuses: a `RUNNING` request is scheduled tokens each step, its prompt's
first and then one at a time.

A finished request may keep its blocks: the connector's `request_finished`
asks to delay the free (`delay_free_blocks`), and the status is finished
all the same (scheduler.py:2623-2624, 2653-2655). NIXL's pull mode keeps
the prefiller's blocks until the decoder has read them, or a lease expires
(nixl/pull_scheduler.py:248-262). The status says nothing of the blocks.

## In serQ

### Before

The interpreter kept two enums. Where a session's statements stand
(src/engine/interp.rs:127-140 at 9bb9417):

```rust
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
    /// At a `join`, until the legs the session forked have ended.
    Joining,
    Ended,
}
```

and how far its engine request has gone (interp.rs:222-235):

```rust
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
```

Neither was reported. The log recorded actions, and a reader had to
reconstruct the state from them, which it could not always do. The first
session of `llmd_nixl_pull.sq` (`serq run
examples/pd-disaggregation/llmd_nixl_pull.sq --workload
examples/workloads/chat_reply.sq --horizon 30 --log log.json`, its events
without the engines' iterations):

```json
{"t": 0, "ev": "arrive", "s": 0}
{"t": 0, "ev": "turn", "s": 0, "turn": 1}
{"t": 0, "ev": "wait", "s": 0, "pool": 0}
{"t": 0, "ev": "admit", "s": 0, "pool": 0}
{"t": 0, "ev": "run", "s": 0, "stages": [0], "mode": "prefill"}
{"t": 0.027420000000000003, "ev": "done", "s": 0, "stage": 0}
{"t": 0.027420000000000003, "ev": "wait", "s": 0, "pool": 6}
{"t": 0.027420000000000003, "ev": "admit", "s": 0, "pool": 6}
{"t": 0.027420000000000003, "ev": "run", "s": 0, "stages": [2, 6]}
{"t": 0.036275, "ev": "done", "s": 0, "stage": 2}
{"t": 0.036275, "ev": "wait", "s": 0, "pool": 4}
{"t": 0.036275, "ev": "admit", "s": 0, "pool": 4}
{"t": 0.036275, "ev": "run", "s": 0, "stages": [4], "mode": "prefill"}
{"t": 0.036477752, "ev": "done", "s": 0, "stage": 4}
{"t": 0.036477752, "ev": "run", "s": 0, "stages": [4], "mode": "decode"}
{"t": 0.052298968, "ev": "done", "s": 0, "stage": 4}
{"t": 0.052298968, "ev": "end", "s": 0}
```

The `run` on stages 2 and 6 is the decoder's connector reading the KV
(vLLM's `WAITING_FOR_REMOTE_KVS`), but nothing says so: the same event is
a plain transfer in `pd_two_fabrics.sq`. The prefiller's blocks stay
leased from 0.0274 until the read is done, and nothing says that either.
A session at a `join` and a `grow` waiting for room were not logged at all
(`docs/reference/log.md`, "Not logged"). A client's think time was a `run`
like an engine's.

### The two enums stay; the state is their image

`Status` and `Phase` are not two spellings of one thing. A connector's
`load` is a block of statements (`kvt.nixl_pull`'s read over two NICs,
`kvt.nixl_push`'s `join` of the prefill leg), run while `Phase` is
`Loading`: the session is `Queued` in a pool, `InStage` at a NIC, or
`Joining`, in that one phase. One enum for both would be their product.
They stay what they are, the interpreter's control, and the state is one
function of them, `Interp::state_of`, which the runtime keeps per session
and per leg (`Session::state`, read again by `Interp::restate` where either
moves):

```rust
pub enum State {
    Waiting,       // queued: a pool's queue (a hold, an engine's admission), a FIFO stage's
    Preempted,     // queued again after a preemption
    Loading,       // its engine's connector's `load` block, whatever it does
    Running,       // a job at a stage; an engine resident's mode beside it
    Transferring,  // a run over shared stages at once, outside a connector
    Saving,        // its engine's connector's `save` block
    Growing,       // a `grow` waiting for room
    Joining,       // at a `join`, waiting for its legs
    Client,        // a run the workload issues: a tool or think time
}
```

`state_of` reads, in this order:

| The session | State | vLLM |
|---|---|---|
| `Ready`, or `Ended` | none: ready takes no time, ended is `end` | |
| `Queued`, its hold asked again after a preemption, or its request preempted since its call | `preempted` | `PREEMPTED` (scheduler.py:1560, 3088-3089 after a load) |
| its request's `Phase` is `Loading` | `loading` | `WAITING_FOR_REMOTE_KVS` (scheduler.py:1267) |
| its request's `Phase` is `Saving` | `saving` | finished, `delay_free_blocks` (scheduler.py:2653-2655) |
| `Joining` | `joining` | |
| `Growing` | `growing` | |
| `Queued` | `waiting` | `WAITING` (request.py:100; scheduler.py:3091 after a load) |
| `InStage`, the `run` the workload's (`Program::sides`) | `client` | |
| `InStage` at a shared stage (`Run.also`, `transfer on`) | `transferring` | |
| `InStage` at a FIFO stage, its work begun and not started | `waiting` | `WAITING` |
| `InStage` otherwise | `running`, with the job's mode | `RUNNING` (scheduler.py:1319) |

The order is the rules: a connector's block is its load or its save,
whatever its statements wait for (a push connector's `join` is the
decoder's request waiting for its KV, not a session joining); a request
asked again after a preemption is preempted until it runs, through its
load and its second admission, as vLLM's is.

Every state is reached in some program or test:

- **waiting** and **preempted** are vLLM's two queued statuses. A hold
  always passes through its pool's queue, even when it is admitted at
  once (the interpreter enqueues, then admits); a FIFO stage's job that
  finds a free server never waits. `waiting` covers both queues: queued
  and not yet served, one meaning.
- **loading** is the connector's `load` block, from the request's
  admission to its blocks until the block is done; the second admission
  that follows, for a running slot, is `waiting` (or `preempted`), as in
  vLLM.
- **running** has the engine's mode as an attribute, not as two states:
  vLLM has one `RUNNING`, and an iteration that chunks a prefill beside
  decodes runs both.
- **transferring** is a run over stages held at once: a fabric's flow.
  Inside a connector it is `loading`; `pd_two_fabrics.sq` transfers in the
  server itself.
- **saving** is the connector's `save` block, before the request's pools
  go back (`vllm_lmcache.sq`'s copy to the L2).
- **growing** and **joining** are serQ's: vLLM preempts rather than waits
  for room, and its proxy, not its scheduler, awaits the legs. No example
  reaches either at its default settings; the tests do
  (`tests/request_state.rs`).
- **client** is a statement of the workload: the turn has been answered
  and the client thinks or calls a tool. vLLM has no request then.
- **finished** is the log's `end`. A refused hold (`refuse`, then `end`) is
  vLLM's request that can never be scheduled, `FINISHED_IGNORED`
  (request.py:401-403).

A leg has its own state, its log events carrying its number. A **lease**
that outlives its hold is not a state: in `llmd_nixl_pull.sq` the session
is `loading` at the decoder while it leases the prefiller's blocks, and in
`vllm_nixl_push.sq` the prefill leg has ended, `finished`, its lease passed
to its session. vLLM keeps the request `FINISHED` and delays the free; the
log keeps the state and says `lease` when the allocation outlives its scope
and `free` when it goes back, by the lease's `id`.

### After

The same run, with its `state`, `lease` and `free` events:

```json
{"t": 0, "ev": "arrive", "s": 0}
{"t": 0, "ev": "turn", "s": 0, "turn": 1}
{"t": 0, "ev": "wait", "s": 0, "pool": 0}
{"t": 0, "ev": "state", "s": 0, "state": "waiting"}
{"t": 0, "ev": "admit", "s": 0, "pool": 0}
{"t": 0, "ev": "run", "s": 0, "stages": [0], "mode": "prefill"}
{"t": 0, "ev": "state", "s": 0, "state": "running", "mode": "prefill"}
{"t": 0.027420000000000003, "ev": "done", "s": 0, "stage": 0}
{"t": 0.027420000000000003, "ev": "lease", "s": 0, "pool": 2, "id": 0}
{"t": 0.027420000000000003, "ev": "wait", "s": 0, "pool": 6}
{"t": 0.027420000000000003, "ev": "state", "s": 0, "state": "waiting"}
{"t": 0.027420000000000003, "ev": "admit", "s": 0, "pool": 6}
{"t": 0.027420000000000003, "ev": "run", "s": 0, "stages": [2, 6]}
{"t": 0.027420000000000003, "ev": "state", "s": 0, "state": "loading"}
{"t": 0.036275, "ev": "done", "s": 0, "stage": 2}
{"t": 0.036275, "ev": "free", "s": 0, "pool": 2, "id": 0}
{"t": 0.036275, "ev": "wait", "s": 0, "pool": 4}
{"t": 0.036275, "ev": "state", "s": 0, "state": "waiting"}
{"t": 0.036275, "ev": "admit", "s": 0, "pool": 4}
{"t": 0.036275, "ev": "run", "s": 0, "stages": [4], "mode": "prefill"}
{"t": 0.036275, "ev": "state", "s": 0, "state": "running", "mode": "prefill"}
{"t": 0.036477752, "ev": "done", "s": 0, "stage": 4}
{"t": 0.036477752, "ev": "run", "s": 0, "stages": [4], "mode": "decode"}
{"t": 0.036477752, "ev": "state", "s": 0, "state": "running", "mode": "decode"}
{"t": 0.052298968, "ev": "done", "s": 0, "stage": 4}
{"t": 0.052298968, "ev": "end", "s": 0}
```

A `state` event follows the action that moved the session there; two in a
row about one session or leg differ. The vLLM oracle's `preempt` scenario
(`serq run tools/oracle/preempt.ir.json --log log.json`), the second
request:

```json
{"t": 0, "ev": "state", "s": 1, "state": "waiting"}
{"t": 0, "ev": "state", "s": 1, "state": "running", "mode": "prefill"}
{"t": 1, "ev": "preempt", "s": 1, "pool": 1}
{"t": 1, "ev": "wait", "s": 1, "pool": 0}
{"t": 1, "ev": "state", "s": 1, "state": "preempted"}
{"t": 30, "ev": "state", "s": 1, "state": "running", "mode": "prefill"}
{"t": 31, "ev": "state", "s": 1, "state": "running", "mode": "decode"}
```

The report gains the states some session or leg was in, after the
client's table (`llmd_nixl_pull.sq`, `--horizon 30`):

```text
state    stays    mean     p50     p99  number
-------  -----  ------  ------  ------  ------
waiting    155  0.0001  0.0000  0.0036   0.000
loading     41  0.0120  0.0106  0.0347   0.016
running    113  0.0369  0.0309  0.1037   0.139
client      56  2.0661  1.6395  7.2678   4.731
```

`stays` are the stays that ended after warm-up, their duration's mean,
median and 99th percentile (within 0.5 %, the stages' `LogHistogram`);
`number` is the time-average sessions and legs in the state. A change of
mode, prefill to decode, is no new stay. Summed over the states, `number`
is `mean_live` in a program without legs, and more with them: a ready
session is in no state for no time (`tests/request_state.rs`). In the
M/M/1 of the getting-started page, `waiting` and `running` are the
queue's wait and service, and add up to the stage's `number`:

```text
state     stays    mean     p50      p99  number
-------  ------  ------  ------  -------  ------
waiting  142452  4.8424  3.3835  21.9678   3.066
running  179174  0.9993  0.6955   4.6061   0.796
```

The JSON has every state, visited or not:
`"states": {"waiting": {"stays", "mean", "p50", "p99", "number"}, …}`;
pyserq's `Report.states` the same, by name.

### What did not move

No syntax and no IR: `IR_VERSION` stays. The state is read from what the
interpreter already keeps, and no expression reads it. The log's version
stays 0 and the report's 1: by their rules (`LOG_VERSION`,
`REPORT_VERSION`, `docs/ir.md` Stability) an added field or an added event
a reader may pass over does not bump them, and no existing field moved.

Every example's report is the same, number for number, apart from the
state table. Checked at seed 1, `--horizon 60`, every program under every
workload `examples/pairs.json` gives it, the tutorial's included (39
runs): the JSON equal with `states` left out, the text equal with the
state table left out. The tutorial and getting-started reports were
reproduced exactly by the previous `serq` and regenerated with the table.

## Every program

Each example and tutorial program under its workload, seed 1, its `state`
events counted by state (each is an entry into the state; `running` with
the modes seen) and its leases; `--horizon 120`, Dai's programs `2e6`
(their clock is 10 µs).

| program | workload | horizon | waiting | preempted | loading | running | transferring | saving | growing | joining | client | leases |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `tutorial/01-queue.sq` | `01-queue.sq` | 120 | 84 |  |  | 99 |  |  |  |  |  |  |
| `tutorial/02-memory.sq` | `02-memory.sq` | 120 | 100 |  |  | 100 |  |  |  |  |  |  |
| `tutorial/03-sessions.sq` | `03-sessions.sq` | 120 | 91 |  |  | 282 |  |  |  |  | 226 |  |
| `tutorial/04-cache.sq` | `04-cache.sq` | 120 | 294 |  |  | 294 |  |  |  |  | 241 |  |
| `tutorial/05-engine.sq` | `05-engine.sq` | 120 | 294 |  |  | 588 (prefill, decode) |  |  |  |  | 241 |  |
| `engines/sglang.sq` | `sglang.sq` | 120 | 480 |  |  | 959 (prefill, decode) |  |  |  |  | 430 |  |
| `engines/tensorrt_llm.sq` | `tensorrt_llm.sq` | 120 | 266 |  |  | 532 (prefill, decode) |  |  |  |  | 235 |  |
| `engines/tgi.sq` | `tgi.sq` | 120 | 266 |  |  | 532 (prefill, decode) |  |  |  |  | 234 |  |
| `multi-turn/closed.sq` | `closed.sq` | 120 | 129 |  |  | 128 |  |  |  |  | 127 |  |
| `multi-turn/replica.sq` | `replica.sq` | 120 | 156 |  |  | 311 (prefill, decode) |  |  |  |  | 155 |  |
| `multi-turn/routing.sq` | `routing.sq` | 120 | 315 |  |  | 948 |  |  |  |  | 853 |  |
| `multi-turn/thunderagent.sq` | `closed_agents.sq` | 120 | 126 | 3 |  | 218 (prefill, decode) |  |  |  |  | 87 | 51 |
| `multi-turn/vllm.sq` | `chat.sq` | 120 | 266 |  |  | 531 (prefill, decode) |  |  |  |  | 235 |  |
| `multi-turn/vllm.sq` | `human_chat.sq` | 120 | 89 |  |  | 178 (prefill, decode) |  |  |  |  | 65 |  |
| `multi-turn/vllm.sq` | `single_turn.sq` | 120 | 399 |  |  | 796 (prefill, decode) |  |  |  |  |  |  |
| `multi-turn/vllm.sq` | `subagents.sq` | 120 | 359 |  |  | 718 (prefill, decode) |  |  |  |  | 255 |  |
| `multi-turn/vllm_lmcache.sq` | `chat.sq` | 120 | 266 |  | 93 | 531 (prefill, decode) |  | 265 |  |  | 234 |  |
| `multi-turn/vllm_system_prompt.sq` | `chat_products.sq` | 120 | 266 |  |  | 531 (prefill, decode) |  |  |  |  | 235 |  |
| `oracle/vllm_request.sq` | `one_request.sq` | 120 | 1 |  |  | 1 (prefill) |  |  |  |  |  |  |
| `oracle/vllm_shared_prompt.sq` | `shared_trace.sq` | 120 | 9 |  |  | 18 (prefill, decode) |  |  |  |  | 8 |  |
| `papers/bari_rad.sq` | `bari_rad.sq` | 120 |  |  |  | 1 (prefill) |  |  |  |  |  |  |
| `papers/dai_fastertransformer.sq` | `dai_periodic.sq` | 2e+06 |  |  |  | 43 (prefill, decode) |  |  |  |  |  |  |
| `papers/dai_sarathi.sq` | `dai_periodic.sq` | 2e+06 |  |  |  | 84 (prefill, decode) |  |  |  |  |  |  |
| `papers/kong_svf.sq` | `kong_batch.sq` | 120 | 200 |  |  | 58 (decode) |  |  |  |  |  |  |
| `pd-disaggregation/llmd_nixl_pull.sq` | `chat_reply.sq` | 120 | 1273 |  | 347 | 1503 (prefill, decode) |  |  |  |  | 519 | 347 |
| `pd-disaggregation/pd_batching.sq` | `pd_batching.sq` | 120 | 21405 |  | 7134 | 21404 (prefill, decode) |  |  |  |  |  | 7134 |
| `pd-disaggregation/pd_open.sq` | `open_poisson.sq` | 120 |  |  |  | 117 |  |  |  |  |  |  |
| `pd-disaggregation/pd_ps.sq` | `pd_ps.sq` | 120 |  |  |  | 1806 |  |  |  |  |  |  |
| `pd-disaggregation/pd_tandem.sq` | `closed_jobs.sq` | 120 | 5575 |  |  | 5298 |  |  |  |  |  |  |
| `pd-disaggregation/pd_two_fabrics.sq` | `pd_two_fabrics.sq` | 120 | 14272 |  |  | 14271 | 7135 |  |  |  |  | 7135 |
| `pd-disaggregation/vllm_nixl_push.sq` | `short_base.sq` | 120 | 861 |  | 287 | 1110 (prefill, decode) |  |  |  |  | 359 | 287 |
| `replay/vllm_replay.sq` | `vllm_replay.sq` | 120 | 257 |  |  | 734 (prefill, decode) |  |  |  |  | 569 |  |
| `single-turn/arguments.sq` | `batch.sq` | 120 |  |  |  | 2 |  |  |  |  |  |  |
| `single-turn/fastertransformer.sq` | `periodic.sq` | 120 | 256 |  |  | 105 (prefill, decode) |  |  |  |  |  |  |
| `single-turn/mg1.sq` | `mg1.sq` | 120 | 79 |  |  | 97 |  |  |  |  |  |  |
| `single-turn/ps.sq` | `ps.sq` | 120 |  |  |  | 88 |  |  |  |  |  |  |
| `single-turn/separate_phases.sq` | `two_prompts.sq` | 120 | 2 |  |  | 4 (prefill, decode) |  |  |  |  |  |  |
| `vendors/ascend.sq` | `ascend_batch.sq` | 120 | 6 |  |  | 16 (prefill, decode) |  |  |  |  |  |  |
| `vendors/rbln.sq` | `rbln_batch.sq` | 120 | 6 |  |  | 12 (prefill, decode) |  |  |  |  |  |  |

Every program is expressed: each is in one of the nine states whenever
time passes (the sum of `number` above). What the table shows of them:

- the engines are `waiting` then `running` (prefill, decode), a
  multi-turn session `client` between turns;
- `preempted` at these settings only in `thunderagent.sq`, whose growth
  preempts; the oracle's `preempt` scenario and the tests reach it in the
  engine programs too;
- `loading` in every connector: the L2's (`vllm_lmcache.sq`) and both NIXL
  modes; the push mode's decoder is `loading` while it `join`s the
  prefill leg, which is why no example is `joining`;
- `transferring` only where the server transfers itself
  (`pd_two_fabrics.sq`): the other P/D programs read the KV in a
  connector;
- the leases are the P/D programs' prefiller blocks and ThunderAgent's
  programs between turns;
- `growing` and `joining` no program reaches here (below).

## Self-critique

**One enum for `Status` and `Phase`.** Asked for, and rejected by the
code: they are orthogonal while a connector's block runs, so one enum
would be their product, and every `match` on where the session is would
have to name the phase it does not care about. The state is the one place
that combines them, `state_of`; the two stay the interpreter's control.

**Prefill and decode as two states.** vLLM has one `RUNNING`, and a chunked
prefill beside decodes is one iteration of both; a request preempted
mid-decode would have to be "decoding" while it prefills again. The mode
is the `running` state's attribute, in the log; the client's `ttft` and
`tpot` and the step stages' fractions already say how the time divides.

**A `request.state` a program reads.** No program needs it: a program that
branches on a preemption reads `request.preempted`, and one that measures
a phase observes the clock. It would be a context variable with moment
rules to settle, an IR change, for no program (CLAUDE.md, "a construct
some program uses").

**A lease as a state (`finished, leased`).** A session leases while it is
elsewhere: `llmd_nixl_pull.sq`'s session is `loading` at the decoder with
the prefiller's blocks leased, so a lease cannot be a state of it without
the two colliding. vLLM itself keeps `FINISHED` and delays the free. A
lease is the `lease` and `free` events; its length is the program's own
observation where it matters (`pd_batching.sq`'s `observe lease`). A
report row for leases was left out: it would be a resource's, not a
state's.

**`transferring` inside a connector.** vLLM's `WAITING_FOR_REMOTE_KVS`
spans the whole wait for the KV, the transfer and what precedes it, so a
connector's block is `loading` throughout. The price: `transferring` is
reached by one example.

**A one-stage NIC run as `transferring`.** The IR does not tell a NIC's
`ps` stage from a compute `ps` stage; only a run over stages held at once,
a fabric's flow, is a transfer by its shape. Marking a stage as a link
would be an IR field for the report alone.

**`ready` as a state.** No time passes in it: a session is ready between
two events of one instant. Kept out, so that the states' `number`s add up
to the sessions live.

**A state at a FIFO stage's queue other than `waiting`.** Queued and not
served is one meaning, whether for a pool's admission or a server; the log
says which (`wait`, or a `run` with no `state running` yet). The asymmetry
that stays: a hold always passes through its pool's queue, so a hold
admitted at once is a `waiting` stay of no time, while a FIFO job that
finds a server free never waits. Dropping the stays of no time was
considered: it would hide the admissions the pool's `wait` counts.

**Replacing the log's `wait` and `run` with `state` events.** Rejected:
serq-lar reads them, and `LOG_VERSION` would bump for no gain. The two
overlap for `waiting` and `running`; the state says what the actions
cannot (`client` against `running`, `loading`, `saving`, `joining`,
`growing`).

**`growing` and `joining` with no example.** Kept: each is a wait of a
construct the language has (`grow` under `preempt none`; `join` outside a
connector) and has a different cause from every other state. Folding
them into `waiting` would make that state mean two things.
