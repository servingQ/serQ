# A request's state

One engine request has exactly one status at any time, by the language's
construction. The request is one call of an engine, `E (input, output)`,
as vLLM's `Request` is one request to one engine; its status is vLLM's:
`waiting`, `waiting_for_kv`, `running`, `preempted`, `finished`. The KV
moves of a request are **transfers**, on an axis of their own: a load, a
save, a send. The run log records every status and every move, and the
report gives the time in each status. A program does not read either.

Getting there changed the language where a request could be in two
states at once: a connector's load and save, which were blocks of
statements, are transfers of a fixed shape; the P/D prefiller's blocks are
kept by a *send* its relation implies, not by a `lease` the program
writes; `join` is gone. IR 15, untagged, carries all of it together with
the prefiller as an engine request (#715, folded in: its `taking … lease
kv (inf)` never shipped, below). A first step, the session's state as one
function of the interpreter's two enums, was built and reverted
([Step 0](#step-0-the-sessions-state-reverted)).

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

## The design

### The request is the engine call

A request is one call of an engine, `E (input, output)` (`CStmt::Request`).
A P/D turn is two: the prefiller's call `P (prompt, 1)` and the decoder's
`D (prompt, produced)`, as vLLM has two `Request`s on two schedulers. A
session is not a request: it is the client, and it may have two requests
at once (a push turn's prefill leg and its decode call) or none (its think
time, a plain stage). A leg (`fork`) is not one either: it is a
session-side thread that calls an engine.

One status per request holds by construction because an engine call's
progress is the engine's (`Req::phase`, `Interp::req_status`) and no
statement of the program runs inside it any more: its connector's moves
are transfers. A program that calls no engine has no request (13 of the
39 example runs: the tutorial's first four, `closed`, `routing`, `mg1`,
`ps`, `arguments`, `pd_open`, `pd_ps`, `pd_tandem`, `pd_two_fabrics`);
its stage and pool tables say where its sessions are.

### Statuses

| status | the request | holds | vLLM |
|---|---|---|---|
| `waiting` | in the engine's waiting queue, not scheduled | nothing, or after a load its blocks | `WAITING` |
| `waiting_for_kv` | admitted to its blocks, not schedulable until its load is done | its blocks, no running slot | `WAITING_FOR_REMOTE_KVS` |
| `running` | a resident of the batch, its mode (prefill, decode) beside it | its blocks and its slot | `RUNNING` |
| `preempted` | in the waiting queue again after a preemption, until it runs | nothing (its tokens) | `PREEMPTED` |
| `finished` | it has generated its output (or was refused) | its blocks while a save or a send of it is registered or in flight | `FINISHED_*` |

`Phase` maps one to one: `Admitted` queued → `waiting` (`preempted` once
preempted); `Loading` → `waiting_for_kv`, and so does a read waiting for
its source's send (`Status::AwaitingSend`); `Waiting` → `waiting`
(`preempted`), as vLLM's request goes back to `WAITING` or `PREEMPTED`
with its blocks once its KV has arrived (scheduler.py:3088-3091);
`Prefill`, `Decode` → `running`; `Saving` → `finished`. The report counts
`finished` only while the blocks are kept: a request that frees them at
its end has no `finished` stay.

**growing** is not a status: a resident stalled for room to grow
(`preempt none`, TGI's and TensorRT-LLM's `kv`) keeps its job among the
engine's residents and is `running`, as a vLLM `RUNNING` request a step
did not schedule is; a `grow` statement waiting for room is outside every
engine call, the session's place. **joining** is not one either: `join`
is gone (below).

### Transfers

| transfer | while the request is | registered | starts | done | vLLM |
|---|---|---|---|---|---|
| `load` | `waiting_for_kv` | at its admission with something to load | when its copy runs (after its source's send, its link, its latency) | at the copy's end | `finished_recving` (scheduler.py:3126-3134) |
| `save` | `finished` | when it has generated its output | when its copy runs (after the L2's room) | at the copy's end | `delay_free_blocks` (lmcache_mp_connector.py:952) |
| `send` | `finished` | at its end, its engine a KV relation's source | when the reader's load copies it | at that load's end, its `ttl`, or the session's end | `finished_sending` (scheduler.py:3135-3138) |

One copy is the source's send and the reader's load. The table is the
only placement the IR can express: a connector's `load` is attached to the
admission, its `save` to the finish, and a send to the source's finish, so
no link check is needed.

**The release rule.** A request's blocks go back when it is `finished`
and no transfer of it is registered or in flight. A send is *registered*
from the source's finish, before the reader has started (vLLM registers
`_reqs_need_send` then, nixl/base_scheduler.py:119-120): "no transfer in
flight" alone would free the prefiller's blocks before the decoder reads
them. A send nobody reads ends at the relation's `ttl` (vLLM's lease,
`_kv_lease_duration`, nixl/pull_scheduler.py:248-262), `inf` by default,
when the session's end does, as the lease did.

## The language changes

### 1. A connector's load and save are transfers

The text already wrote a fixed shape; only the IR was more general.
`vllm_lmcache.sq` (unchanged):

```serq
    connector lmc {
      per_token (r);
      load (request.preempted ? 0 : beyond(request.known, request.hit));
      save (floor(request.input + request.output, chunk));
    }
```

Before, IR 14 at 9bb9417 (`serq ir`, the statements of each block):

```text
load:  Run lmc (loaded * r); Load kv (loaded)
save:  Hold lmc.kv { Grow lmc.kv (saved - cached); Run lmc (…); Load lmc.kv (…) }
```

and `kvt.nixl_pull`'s reader `Run P.nic[from], D.nic (loaded, latency x0,
share maxmin); Load D.kv (loaded); Release P.kv[from]`, `kvt.nixl_push`'s
the same with `Join` first. While such a block ran the request was
`Loading` and the session `Queued`, `InStage` at a NIC or `Joining`: two
states. After, `CConnector.load` and `.save` are a `CTransfer` each
(`serq ir examples/pd-disaggregation/llmd_nixl_pull.sq`, `D`'s load):

```json
{"amount": {"Cond": [{"Ctx": "Requeued"}, {"Num": 0.0}, {"Binary": ["Sub", {"Ctx": "Input"}, {"Ctx": "Hit"}]}]},
 "stage": {"base": 2, "count": 2, "index": {"Attr": 20}},
 "also": [{"base": 6, "count": 2, "index": {"Num": 0.0}}],
 "work": {"Cost": [{"Joint": [{"Stage": {"base": 2, "count": 2}}, {"Stage": {"base": 6, "count": 2}}]}, {"Attr": 30}]},
 "share": "MaxMin", "latency": {"Num": 0.002}, "poster": 1,
 "into": {"base": 6, "count": 1, "index": null}, "units": {"Cost": [{"Pool": {"base": 6, "count": 1}}, {"Attr": 30}]},
 "source": {"base": 2, "count": 2, "index": {"Attr": 20}}}
```

and an L2's save has `room: {held, grow, cache}` where the read has
`source`. The engine runs a transfer as its own action;
`Program::kernel` writes the kernel statements it is for the interpreter
and the figure, which the IR no longer holds. A read waits for its
source's send, which a push's leg registers, and ends it.

### 2. The relation's send replaces the request's lease

Before, on main, the prefiller's entry is a hold that leases:

```serq
    prefill (prompt) {
      hold reqs (cost(reqs, 1)), kv (cost(kv, min(prompt, reused + budget_left(P)))) reserve (cost(kv, prompt))
           at admission (reused = min(cachedin(kv), floor(prompt - 1, bs))) {
        set c = cached;
        run P prefill (cost(P, prompt - c)) growing kv;
      } cache (cost(reqs, kv, prompt)) lease kv (inf);
    }
```

and #715 made it an engine request whose `taking` ended `cache
(request.input) lease kv (inf);` (`CRequest.lease`). After, the entry is
`P (prompt, 1);` and the `taking` ends `cache (request.input);`: the
relation `kvt.nixl_pull (P, D, latency = x0, share = share.maxmin);`
already says that `P`'s requests are read, and the linker sets
`CRequest.send: {ttl}` on the source engine (`ttl = t` an optional
argument of the relation, `inf` by default). The reader's lowered
`Release P.kv[from]` goes: the read ends the send. Which send: the latest
the session holds of that member, its source's call the last before the
read; no example ever holds two at a read (checked over `llmd_nixl_pull`
2000 s, `pd_batching` 600 s, `vllm_nixl_push` 2000 s: one each time), so
matching by call moves no report.

**Why the lease spelling never shipped.** `lease kv (inf)` on a request
wrote, in the program, a fact the relation already states (the source's
requests are read), and its `inf` was a policy no program wrote the
opposite of: a `lease` with no reader was refused, and with a reader the
reader's connector ended it. The only parameter, how long an unread send
lasts, is a setting of the relation in vLLM (`kv_lease_duration`), so it
went there as `ttl`. #715's prefiller-as-engine-request stays whole; only
its last clause went (`docs/design/prefiller-engine-request.md`).

### 3. `join` goes

`vllm_nixl_push.sq` (unchanged):

```serq
      if (concurrent) {
        fork { P.prefill (prompt); }     // asyncio.create_task: the prefill leg runs beside
      } else {
        P.prefill (prompt);              // the sidecar's order: the prefill's answer first
      }
      D.decode (prompt) from P;          // the decode request; it waits for the prefill's KV inside
```

Before, the decoder's connector block began with a `Join`. After, the
read waits for the leg's send (the session's place `AwaitingSend`, the
request's status `waiting_for_kv`, as vLLM's decoder waits for the push),
woken when the leg ends and its send passes to the session; in a pull,
with no leg live, a read whose send has expired reads nothing there.
`CStmt::Join` and `join;` go: the parser refuses `join;` saying what
replaced it. The fork rule "a program that forks must join" becomes what
the language now needs: a leg calls, on every way through it, an engine
whose requests are sent, since the session waits for a leg only by reading
its send; a session ending while a leg runs stays a run-time error.
`E-pull-forked` goes too: a forked pull's reader waits for the send as a
push's does, and runs the push's numbers (`tests/copy_diagnostics.rs`).

### IR

IR 15, untagged (14 is tagged, v0.1.7-alpha.1):

- `CConnector.load`, `.save`: `(CExpr, BlockId)` → `CTransfer {amount,
  stage, also, work, share?, latency?, poster?, into, units, room?,
  source?}`, with `CRoom {held, grow, cache}`;
- `CRequest.send?: CSend {ttl}`, set by the linker on a relation's source;
- `CStmt::Join` removed;
- `CRequest.lease` (#715) never added.

`scripts/gen_lean_oracle.py` already refused a connector and refuses a
send; no oracle program had a `Join`: no program leaves the fragment,
`make lean` passes. No oracle, claims or golden IR file moves (none has a
connector). `tools/metrics.json`: `CStmt` 18 → 17.

## What moved

Every pair in `examples/pairs.json` (39 runs, seed 1, `--horizon 60`)
against origin/main's `serq` and #715's: the JSON is #715's, field for
field, without `status`, and main's but for the three P/D programs'
prefiller records #715 adds (`P.reused`, `P.prefill`; `vllm_nixl_push.sq`
drops its hand-written `p_cached`); the text removes no line of #715's and
adds the status table. The tutorial's fifth chapter's report gains the
table; the others call no engine and are unchanged.

The tests that used `fork { … } join;` as a device for concurrency
(`run_latency`, two sessions now) or for a leg's hold
(`client_metrics::a_leg_works_for_the_turn_it_was_forked_in`,
`lease_preemption::a_lease_passed_from_a_leg_can_be_taken`,
`shared_prefix::two_legs_read_one_prompt` and the leg half of
`cachedin_outside_a_hold_reads_the_chain_cached_along`) went with `join`:
a leg that holds rather than calls a source is no longer a program.

## Every program

Each example and tutorial program, seed 1, its `status` events counted by
status (entries; `running` with its modes) and its transfers by kind;
`--horizon 120`, Dai's programs `2e6` (their clock is 10 µs). Blank: none.

| program | workload | horizon | requests | waiting | waiting_for_kv | running | preempted | finished | load | save | send |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `tutorial/01-queue.sq` | `01-queue.sq` | 120 |  |  |  |  |  |  |  |  |  |
| `tutorial/02-memory.sq` | `02-memory.sq` | 120 |  |  |  |  |  |  |  |  |  |
| `tutorial/03-sessions.sq` | `03-sessions.sq` | 120 |  |  |  |  |  |  |  |  |  |
| `tutorial/04-cache.sq` | `04-cache.sq` | 120 |  |  |  |  |  |  |  |  |  |
| `tutorial/05-engine.sq` | `05-engine.sq` | 120 | 294 | 294 |  | 588 (prefill, decode) |  | 294 |  |  |  |
| `engines/sglang.sq` | `sglang.sq` | 120 | 480 | 480 |  | 959 (prefill, decode) |  | 479 |  |  |  |
| `engines/tensorrt_llm.sq` | `tensorrt_llm.sq` | 120 | 266 | 266 |  | 532 (prefill, decode) |  | 266 |  |  |  |
| `engines/tgi.sq` | `tgi.sq` | 120 | 266 | 266 |  | 532 (prefill, decode) |  | 265 |  |  |  |
| `multi-turn/closed.sq` | `closed.sq` | 120 |  |  |  |  |  |  |  |  |  |
| `multi-turn/replica.sq` | `replica.sq` | 120 | 156 |  |  | 311 (prefill, decode) |  | 155 |  |  |  |
| `multi-turn/routing.sq` | `routing.sq` | 120 |  |  |  |  |  |  |  |  |  |
| `multi-turn/thunderagent.sq` | `closed_agents.sq` | 120 | 107 | 107 |  | 218 (prefill, decode) | 3 | 97 |  |  |  |
| `multi-turn/vllm.sq` | `chat.sq` | 120 | 266 | 266 |  | 531 (prefill, decode) |  | 266 |  |  |  |
| `multi-turn/vllm.sq` | `human_chat.sq` | 120 | 89 | 89 |  | 178 (prefill, decode) |  | 89 |  |  |  |
| `multi-turn/vllm.sq` | `single_turn.sq` | 120 | 399 | 399 |  | 796 (prefill, decode) |  | 399 |  |  |  |
| `multi-turn/vllm.sq` | `subagents.sq` | 120 | 359 | 359 |  | 718 (prefill, decode) |  | 359 |  |  |  |
| `multi-turn/vllm_lmcache.sq` | `chat.sq` | 120 | 266 | 266 | 93 | 531 (prefill, decode) |  | 265 | 93 | 265 |  |
| `multi-turn/vllm_system_prompt.sq` | `chat_products.sq` | 120 | 266 | 266 |  | 531 (prefill, decode) |  | 266 |  |  |  |
| `oracle/vllm_request.sq` | `one_request.sq` | 120 | 1 | 1 |  | 1 (prefill) |  | 1 |  |  |  |
| `oracle/vllm_shared_prompt.sq` | `shared_trace.sq` | 120 | 9 | 9 |  | 18 (prefill, decode) |  | 9 |  |  |  |
| `papers/bari_rad.sq` | `bari_rad.sq` | 120 | 1 |  |  | 1 (prefill) |  |  |  |  |  |
| `papers/dai_fastertransformer.sq` | `dai_periodic.sq` | 2e+06 | 42 |  |  | 43 (prefill, decode) |  |  |  |  |  |
| `papers/dai_sarathi.sq` | `dai_periodic.sq` | 2e+06 | 42 |  |  | 84 (prefill, decode) |  |  |  |  |  |
| `papers/kong_svf.sq` | `kong_batch.sq` | 120 | 58 |  |  | 58 (decode) |  | 33 |  |  |  |
| `pd-disaggregation/llmd_nixl_pull.sq` | `chat_reply.sq` | 120 | 926 | 1273 | 347 | 1503 (prefill, decode) |  | 926 | 347 |  | 347 |
| `pd-disaggregation/pd_batching.sq` | `pd_batching.sq` | 120 | 14271 | 21405 | 7134 | 21404 (prefill, decode) |  | 14268 | 7134 |  | 7134 |
| `pd-disaggregation/pd_open.sq` | `open_poisson.sq` | 120 |  |  |  |  |  |  |  |  |  |
| `pd-disaggregation/pd_ps.sq` | `pd_ps.sq` | 120 |  |  |  |  |  |  |  |  |  |
| `pd-disaggregation/pd_tandem.sq` | `closed_jobs.sq` | 120 |  |  |  |  |  |  |  |  |  |
| `pd-disaggregation/pd_two_fabrics.sq` | `pd_two_fabrics.sq` | 120 |  |  |  |  |  |  |  |  |  |
| `pd-disaggregation/vllm_nixl_push.sq` | `short_base.sq` | 120 | 574 | 861 | 287 | 822 (prefill, decode) |  | 574 | 287 |  | 287 |
| `replay/vllm_replay.sq` | `vllm_replay.sq` | 120 | 257 | 257 |  | 477 (prefill, decode) |  | 257 |  |  |  |
| `single-turn/arguments.sq` | `batch.sq` | 120 |  |  |  |  |  |  |  |  |  |
| `single-turn/fastertransformer.sq` | `periodic.sq` | 120 | 102 |  |  | 105 (prefill, decode) |  | 2 |  |  |  |
| `single-turn/mg1.sq` | `mg1.sq` | 120 |  |  |  |  |  |  |  |  |  |
| `single-turn/ps.sq` | `ps.sq` | 120 |  |  |  |  |  |  |  |  |  |
| `single-turn/separate_phases.sq` | `two_prompts.sq` | 120 | 2 | 2 |  | 3 (prefill, decode) |  | 2 |  |  |  |
| `vendors/ascend.sq` | `ascend_batch.sq` | 120 | 6 | 6 |  | 12 (prefill, decode) |  | 6 |  |  |  |
| `vendors/rbln.sq` | `rbln_batch.sq` | 120 | 6 | 6 |  | 12 (prefill, decode) |  | 6 |  |  |  |

- one engine call per turn everywhere an engine is called; P/D turns two;
- `preempted` at these settings only in `thunderagent.sq`;
- `waiting_for_kv` with a `load` in every connector, a `save` only in
  `vllm_lmcache.sq`, a `send` per remote prefill in the three P/D programs;
- `finished` above is the log's event, one per request that ended within
  the horizon (`fastertransformer.sq` and `kong_svf.sq` end with most of
  theirs running); the report's `finished` counts only the time a save or
  a send kept the blocks.

## Decisions

1. **#715 folds in.** Its prefiller as an engine request stays; its
   `lease` clause does not ship; one IR version carries both.
2. **An unread send ends at the relation's `ttl`**, `inf` by default
   (the session's end, the reports as before).
3. **`join` goes**, refused with what replaced it; a leg calls a source
   engine; a session ending before its legs stays a run-time error.
4. **No session positions in the report** (P1): the status table is per
   engine request; the log's actions (`wait`, `admit`, `run`, `done`) are
   the positions.
5. **Step 0 reverted**: its `state`, `lease` and `free` events (never
   released) became `status` and `transfer`, without a `LOG_VERSION`
   bump; its report table became `status`, an added field
   (`REPORT_VERSION` stays 1).

## Self-critique

**The session (or the turn) as the request.** It covers every example,
but a push turn has the prefill and the decode at once, and a pull turn
the prefiller's finished request with its send while the decoder waits for
KV: one status for the turn is false there.

**A function of `Status` and `Phase`** (Step 0). Two control variables
overlap while a connector's block runs; their image is one state only by a
priority order the reader has to learn.

**Keeping the connector's blocks and checking them at link time.** The
check would be the type `CTransfer` written as a predicate over
statements, for a shape the text already fixes.

**`loading`, `saving`, `transferring` as statuses.** Each is a status and
a transfer at once; vLLM keeps them apart for the same reason.

**A request's `lease` (#715) or a lease as a status.** Above: the
relation says it, and a lease that outlives its request is a resource of
it, not where it is.

**`join` kept as a session position, or folded into `waiting`.** No
program wrote `join` once the push connector no longer did; and `waiting`
would mean a queue and a child (criterion 0).

**A `fork` of any body, with `join` gone.** A session cannot wait for a
leg that leaves it nothing to read, so such a leg would outlive it on the
first turn that does not wait by chance; the run-time error would be the
only check. The leg's body is now refused statically.

**Keeping `E-pull-forked`.** Its reason was that a pull's reader waited
for nothing; it now waits for the send. A forked pull is not vLLM's shape
(the decoder pulls with the prefiller's answer), but which shapes a program
models is its policy (criterion 2).

**Session positions in the report (P2).** A second table of where
sessions are (queued, served, transferring, client) would overlap the
stage and pool tables and be confused with the statuses; not done.

**`growing` as a status.** A stalled resident is in the batch; a waiting
`grow` statement outside an engine call is a session's place.

**A `ttl` that reads the request.** vLLM's lease is a setting; a constant
on the relation, read once.

## Step 0: the session's state, reverted

The first step, built and then reverted: one state per session and leg,
read from the interpreter's `Status` and `Phase`. Kept here for its
evidence (the two enums, the overlaps it found, the per-example table);
the design above replaces it.

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
