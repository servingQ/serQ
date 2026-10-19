# A request's state

**Status: design, for approval.** One request has exactly one status at any
time, by the language's construction. The statuses are vLLM's, the
request is the engine call `E (input, output)`, and the KV moves a
connector makes are **transfers** attached to a request, on an axis of
their own. A first step, the session-level image of the interpreter's two
enums, is implemented on this branch (commit 9ebbf0c, [Step 0](#step-0-implemented-on-this-branch-the-sessions-state)
below); this document says what replaces it and which language changes
make one status per request true. Nothing of the language changes is
implemented yet.

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

**The transfers are a second axis.** A finished request may keep its
blocks: the connector's `request_finished` asks to delay the free
(`delay_free_blocks`), and the status is finished all the same
(scheduler.py:2623-2624, 2653-2655). The blocks go when the worker reports
the request's send done, or its receive done for a request that finished
while it received (`finished_sending`, `finished_recving`,
scheduler.py:3126-3138). NIXL registers the send at the prefiller's finish,
before the decoder has started to read, with an expiry
(nixl/pull_scheduler.py:248-262; `_reqs_need_send`,
nixl/base_scheduler.py:119-120). LMCache's connector keeps a finished
request's blocks until they are copied (lmcache_mp_connector.py:952). The
status never says a transfer is in flight; the connector's sets do.

In P/D, the prefiller's request and the decoder's request are two
`Request`s on two schedulers, each with its own status: the prefiller's is
finished, its blocks kept for the send, while the decoder's is
`WAITING_FOR_REMOTE_KVS`.

## The design: one status, and transfers beside it

### The request is the engine call

A request is one call of an engine, `E (input, output)` (`CStmt::Request`),
as vLLM's `Request` is one request to one engine. A P/D turn is two: the
prefiller's call and the decoder's. A session is not a request: it is the
client, and it may have two requests at once (a push P/D turn's prefill
leg and its decode call), or none (its think time, a plain stage). A leg
(`fork`) is not a request either: it is a session-side thread that calls
an engine.

This is the unit for which one status is true by construction: an engine
call's progress is the engine's (`Req::phase`), and the only statements
that run inside it are its connector's blocks, which the first change below
turns into transfers.

What each example's requests are:

| program | requests (engine calls) | statuses reached | transfers |
|---|---|---|---|
| `tutorial/05-engine.sq`, `engines/*`, `multi-turn/vllm.sq` (4 workloads), `vllm_system_prompt.sq`, `replica.sq`, `oracle/*`, `replay/vllm_replay.sq`, `papers/*`, `single-turn/fastertransformer.sq`, `separate_phases.sq`, `vendors/*` | one call per turn, of the one engine or a replica | waiting, running, finished (some never wait) | none |
| `multi-turn/thunderagent.sq` | one call per turn, of replica `E[home]` | waiting, running, preempted, finished | none |
| `multi-turn/vllm_lmcache.sq` | one call per turn | waiting, waiting_for_kv, running, finished | load (from the L2), save (to the L2) |
| `pd-disaggregation/llmd_nixl_pull.sq` | a remote turn: the prefiller's call, then the decoder's; a local turn: the decoder's | waiting, waiting_for_kv, running, finished | send (prefiller) = load (decoder), one copy |
| `pd-disaggregation/pd_batching.sq` | the prefiller's call, then the decoder's | as above | as above |
| `pd-disaggregation/vllm_nixl_push.sq` | the prefiller's call (in a leg, concurrent) and the decoder's | as above | as above, the source posting |
| `tutorial/01`–`04`, `multi-turn/closed.sq`, `routing.sq`, `single-turn/mg1.sq`, `ps.sq`, `arguments.sq`, `pd_open.sq`, `pd_ps.sq`, `pd_tandem.sq`, `pd_two_fabrics.sq` | none: no engine | | |

The P/D prefillers are engine calls only once #715 lands
(`feat/prefiller-engine-request`): on main they are a `hold … { run P
prefill … } lease kv (inf)` in the session, the kernel spelling of an
engine request. This design builds on #715.

### Statuses

| status | the request | holds | vLLM |
|---|---|---|---|
| `waiting` | is in the engine's waiting queue, not scheduled | nothing, or the blocks of a load it has received | `WAITING` |
| `waiting_for_kv` | is admitted to its blocks and not schedulable until its load is done | its blocks, no running slot | `WAITING_FOR_REMOTE_KVS` |
| `running` | is a resident of the engine's batch; its mode, prefill or decode, is an attribute | its blocks and its slot | `RUNNING` |
| `preempted` | is in the waiting queue again after a preemption, until it runs | nothing (its tokens) | `PREEMPTED` |
| `finished` | has generated its output, or was refused | its blocks while a transfer of it is in flight | `FINISHED_*` |

A `waiting` request may hold blocks: after its load it waits for a slot
(`request_more`), as vLLM's request goes back to `WAITING` with its blocks
(scheduler.py:3088-3091). So `waiting` is "in the waiting queue", not "not
admitted"; the candidate's wording is corrected here.

Today's `Phase` maps one to one: `Admitted` queued → `waiting` (or
`preempted`); `Loading` → `waiting_for_kv`; `Waiting` → `waiting` (or
`preempted`); `Prefill`, `Decode` → `running`; `Saving` → `finished`
with a save in flight.

### Transfers

A transfer is a KV move attached to a request, with its own time: a job
on the stages it crosses (a NIC pair, an L2's stage), and the destination
room it may wait for.

| transfer | of the request | while its status is | ends | vLLM |
|---|---|---|---|---|
| `load` | the KV it reads in, from an L2 or a source request's send | `waiting_for_kv` | the copy done: the request `waiting` | `finished_recving` (scheduler.py:3126-3134) |
| `save` | the KV it writes to an L2 | `finished` (vLLM may also save while `running`; no serQ connector does) | the copy done | `delay_free_blocks` (lmcache_mp_connector.py:952) |
| `send` | its KV, kept for a reader | `finished` | the reader's load done | `finished_sending` (scheduler.py:3135-3138) |

A transfer has a lifecycle of its own: registered, moving, done. A send is
registered when its request finishes and moves only when its reader's load
does (the same copy is the source's send and the reader's load). The
status table above is the only placement the IR can express: the
connector's `load` is attached to the admission, its `save` to the finish,
and a relation's `send` to the source's finish (the first change below), so
no link check is needed: the IR has no place to write a load elsewhere.

**The release rule.** A request's blocks go back when it is `finished`
and no transfer of it is registered or in flight. This replaces a
request's lease: the prefiller's blocks stay because its send is
registered, not because the program wrote `lease kv (inf)`.

The overlaps the earlier step reported as one state become a status and a
transfer:

| was (Step 0) | is |
|---|---|
| `loading` | `waiting_for_kv` + a `load` |
| `saving` | `finished` + a `save` |
| a prefiller's lease past its request | `finished` + a `send` |
| `transferring` in a connector | the transfer itself |

### What is not a status

**growing.** A `growing` run stalled for room (`preempt none`: TGI's and
TensorRT-LLM's `kv`) keeps its job among the engine's residents
(`Status::Growing(…, Some(job))`): the request is `running`, not
advancing, as a vLLM `RUNNING` request the step did not schedule is.
A `grow` statement waiting for room is outside every engine call (the
L2 save's `grow` becomes part of the `save` transfer): a session position,
not a request's status. No language change.

**joining.** A `join` is the session waiting for its legs: a session
position. Its one use is the push connector's lowered block (below), which
the first change removes; then no program writes `join` (open decision 3).

**client, transferring outside an engine, a plain stage.** The session's
position. 13 of the 39 example runs make no engine call at all.

## The language changes

### 1. A connector's load and save are transfers, not blocks

The program text already writes a fixed shape; only the IR is more
general than any text can be. `vllm_lmcache.sq` (at 9bb9417):

```serq
    connector lmc {
      per_token (r);
      load (request.preempted ? 0 : beyond(request.known, request.hit));
      save (floor(request.input + request.output, chunk));
    }
```

lowers to two statement blocks (`serq ir`, the statements of each):

```text
load:  Run lmc (loaded * r); Load kv (loaded)
save:  Hold lmc.kv { Grow lmc.kv (saved - cached); Run lmc (…); Load lmc.kv (…) }
```

and the KV relation of `llmd_nixl_pull.sq`, `kvt.nixl_pull (P, D, latency =
x0, share = share.maxmin);` with `connector P { load (request.preempted ?
0 : request.input - request.hit); }`, to:

```text
load:  Run P.nic[from], D.nic (loaded, latency x0, share maxmin); Load D.kv (loaded); Release P.kv[from]
```

and `vllm_nixl_push.sq`'s (`kvt.nixl_push`, a leg forked) to the same
with `Join` first (`src/frontend/link.rs`, the connector's lowering:
"the source's KV is there once its leg has ended").

While such a block runs the request's `Phase` is `Loading` or `Saving`
and the session is `Queued`, `InStage` at a NIC or `Joining`: two states at
once. After, `CConnector.load` and `.save` are a transfer each, of a fixed
shape the interpreter runs as the engine's own action:

```rust
pub struct CTransfer {
    pub amount: CExpr,            // the tokens, as today's expression
    pub via: Vec<CRef>,           // the stages crossed: an L2's, or the two NICs
    pub cost: CExpr,              // the work per token on them (`per_token`, or 1)
    pub latency: Option<CExpr>,   // a relation's wait before the copy
    pub share: Option<Share>,     // a fabric's policy, with two stages
    pub poster: Option<usize>,    // which side posts it (the view's dot)
    pub peer: Peer,               // L2 (its pool, cached) or Source (the relation's queue, `from`)
}
```

The request is `waiting_for_kv` from its admission to the load's end,
whatever the copy waits for; `finished` while its save runs, the save
taking the L2's room itself (today's `Hold`/`Grow`). The program text does
not change.

- **IR**: `CConnector.load`/`.save` retyped from `(CExpr, BlockId)` to
  `CTransfer`: a version, the blocks they named go. `Run.poster` leaves
  `Run` if only the connector set it (to check). `scripts/gen_lean_oracle.py`
  already refuses a connector (`Fragment("an engine's connector")`): no
  Lean program moves, the generator's pin moves.
- **Examples**: `vllm_lmcache.sq`, `llmd_nixl_pull.sq`, `pd_batching.sq`,
  `vllm_nixl_push.sq`; text unchanged, IR changed.
- **Reports**: expected identical: the transfer does today's statements in
  today's order. To be checked line for line when implemented.

### 2. A source request's send replaces its lease

With #715, `llmd_nixl_pull.sq`'s prefiller is

```serq
          taking reqs (1),
                 kv (min(request.known, request.hit + budget_left(P))) reserve (request.known)
          reuse (floor(request.known - 1, bs))
          cache (request.input)
          lease kv (inf);
    …
    prefill (prompt) {
      P (prompt, 1);
    }
```

(on main, the `hold … { run P prefill … } lease kv (inf)` it replaces), and
the reader's connector ends the lease with `Release P.kv[from]`. After,
the relation `kvt.nixl_pull (P, D, …)` already says that P's requests are
read: a P request registers a send at its finish, its blocks stay by the
release rule, and the reader's load ends it. `lease kv (inf)` goes from the
text and `CRequest.lease` from the IR; the lowered `Release` goes.

- **IR**: `CRequest.lease`, which #715 adds as IR 15, is not needed. IR 14
  is tagged (v0.1.7-alpha.1); while 15 is untagged, #715 and this change
  are one version (`CLAUDE.md`, IR rules): best decided before #715
  merges (open decision 1).
- **Examples**: the three P/D programs.
- **Where the candidate fails, and the fix.** (a) A send must exist from
  its source's finish, before the reader starts: "no transfer in flight"
  alone would free the prefiller's blocks at its finish. Fix: a send is
  registered at the finish (vLLM registers `_reqs_need_send` then). (b) A
  send nobody reads: a decoder whose `load (request.preempted ? 0 :
  request.input - request.hit)` is 0 (preempted, or its own hit covers the
  prompt) runs no load, so nothing ends the send. Today such a lease lasts until the session's end. Options:
  end it at the session's end (reports identical), at the reader's
  admission that loads nothing, or at an expiry the relation names
  (`ttl`, vLLM's lease); open decision 2. (c) A session with two
  registered sends to one source member: today's `Release` ends the
  oldest lease (`release_early` → `lease_of`), which need not be the one
  just read; a send is matched to its reader by the turn that made both.
  Whether any example reaches two at once is to be checked when
  implementing; if one does, its report moves, and that is a fix.

### 3. The push mode needs no `join`

`vllm_nixl_push.sq` (at 9bb9417):

```serq
      if (concurrent) {
        fork { P.prefill (prompt); }     // asyncio.create_task: the prefill leg runs beside
      } else {
        P.prefill (prompt);              // the sidecar's order: the prefill's answer first
      }
      D.decode (prompt) from P;          // the decode request; it waits for the prefill's KV inside
```

The decoder's load waits with a `Join` for the leg. After, it waits for
the source request's send to be registered: the decoder is
`waiting_for_kv` until the prefiller has finished and the copy is done,
as vLLM's decoder waits for the push. The `fork` stays (the client sends
both at once). The text does not change.

- **IR**: no lowered `Join`. The statement `join;` then has no program
  (the corpus's one use was this lowering): open decision 3.
- **Reports**: expected identical: the leg ends at its call's finish, the
  instant its send is registered.

### 4. `growing` and `joining` as statuses go

No language change for `growing` (a stalled resident is `running`). For
`joining`, change 3 and open decision 3.

## Session positions

The user's first brief wanted coverage beyond the engine. A session's
position (a pool's queue, a plain stage, a run over a fabric, the client's
tool, a `join`, a waiting `grow`, in an engine call) is a different thing
from a request's status, and is not made one. Options:

- **P1, no positions in the report.** The log keeps its actions (`wait`,
  `admit`, `run`, `done`), which are the positions; the report keeps its
  stage and pool tables and the client's metrics. Cost: the 13 runs with no
  engine have no state table; nothing new to maintain.
- **P2, a `position` table beside the `status` table.** Step 0's code,
  renamed and narrowed: `queued`, `served`, `transfer`, `client`,
  `joining`, `growing`, and `engine` (in a call: its status is in the
  other table). Cost: two tables a reader must not confuse; the
  positions overlap the stage and pool tables.

The log: `status` events per request with a request id (a session's
finished prefiller with a registered send and its decoder waiting for KV
are two requests of one session at once), and `transfer` events (kind,
registered, moving, done). Step 0's `state`, `lease` and `free` events
were never released: they can be renamed or dropped without a
`LOG_VERSION` bump.

## Open decisions

1. **Order with #715.** Land #715 with `CRequest.lease` and remove it here
   (two changes to one untagged IR 15), or fold change 2 into #715 before
   it merges.
2. **An unread send.** End it at the session's end (reports identical), at
   the reader's admission that loads nothing, or at a relation's `ttl`
   (vLLM's). The latter two move the P/D reports wherever a decoder is
   preempted before its read.
3. **`join;`.** Remove it (`CStmt::Join`, a version, no program uses it
   after change 3; `docs/design/subagents.md`'s fan-in would come back with
   its own design), keeping today's run-time error for a session ending
   before its legs, or letting such legs run on as orphans (the refused
   session's rule today). Or keep it as a session position.
4. **Session positions**: P1 or P2.
5. **Step 0's code** on this branch: superseded by the implementation of
   this design, kept as P2's base, or reverted before merge.

## Self-critique (the design)

**The session (or the turn) as the request.** It covers every example,
but it is not vLLM's unit: a push P/D turn has the prefill and the decode
at once, and a pull turn has the prefiller's finished request with its send
while the decoder waits for KV. One status for the turn is false there.

**A function of `Status` and `Phase`** (Step 0). Two control variables
overlap while a connector's block runs; their image is one state only by a
priority order the reader has to learn. Rejected for the request; kept at
most as the session's position (P2).

**Keeping the connector's blocks and checking them at link time.** A
check that a connector's block only copies is the type `CTransfer` written
as a predicate over statements. The text already has the fixed shape.

**`saving`, `loading`, `transferring` as statuses.** Each is a status and a
transfer at once: `loading` is waiting for KV and a load, `saving` is
finished and a save. vLLM keeps them apart for the same reason.

**A lease as a status, or `lease kv (inf)` kept on a request.** A lease
the program writes says what the relation already says (P's requests are
read), and its `inf` is the policy no program states the opposite of
(criterion 2 is met by the `ttl` option, not by the clause). A hold's
`lease` stays: ThunderAgent's `prog` and `pd_two_fabrics.sq`'s `kvP` are
session allocations, not requests.

**`join` folded into `waiting`.** "Waiting" would mean a queue and a
child, two meanings (criterion 0).

**A `grow` that waits refused at link time.** Possible (no example's
`grow` statement waits after change 1), but it is a session position, not
a request's status, and refusing it is not needed for one status per
request.

## Step 0, implemented on this branch: the session's state

What commit 9ebbf0c implements, before the design above: one state per
session and leg, read from the interpreter's `Status` and `Phase`. Kept
here for its evidence (the two enums, the overlaps, the per-example
table); the design above supersedes it, and its `growing` and `joining`
go ("What is not a status").

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

### Every program (Step 0)

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

### Self-critique (Step 0)

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
