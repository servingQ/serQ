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
them. A send no copy has begun to read ends at the relation's `ttl`
(vLLM's lease, `_kv_lease_duration`, nixl/pull_scheduler.py:248-262), or,
without one, at the session's end, as the lease did; a reader that comes
after computes the tokens itself (4).

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
woken when the leg ends and its send passes to the session, or when no
leg is left to leave one (`send_ready`, the one rule both places read).
`CStmt::Join` and `join;` go: the parser refuses `join;` saying what
replaced it. The fork rule "a program that forks must join" becomes what
the language now needs, all checked when the program links: a leg calls,
once on every way through it, an engine whose requests are sent, and
leases nothing, since the session waits for a leg only by reading its
send; and every way of the session after the `fork` reaches a call whose
connector reads that source before the session ends. A session ending
while a leg runs stays a run-time error behind those checks.
`E-pull-forked` goes too: a forked pull's reader waits for the send as a
push's does, and runs the push's numbers (`tests/copy_diagnostics.rs`).

### IR

IR 15, untagged (14 is tagged, v0.1.7-alpha.1):

- `CConnector.load`, `.save`: `(CExpr, BlockId)` → `CTransfer {amount,
  stage, also, work, share?, latency?, poster?, into, units, room?,
  source?}`, with `CRoom {held, grow, cache}`;
- `CRequest.send?: CSend {ttl?: f64}`, set by the linker on a relation's
  source, the `ttl` a constant above 0 (absent: none);
- `CStmt::Join` removed; a `Fork`'s body calls a source once and leases
  nothing, and its send is read on every way after it (validation);
- `CRequest.lease` (#715) never added.

`scripts/gen_lean_oracle.py` already refused a connector and refuses a
send; no oracle program had a `Join`: no program leaves the fragment,
`make lean` passes. No oracle, claims or golden IR file moves (none has a
connector). `tools/metrics.json`: `CStmt` 18 → 17.

### 4. A send that has ended is not read

A send registered at the source's end lasts until a reader's copy begins
to read it, then until the copy is done (vLLM's decoder renews the lease
while it reads, `nixl/base_scheduler.py:199-238`). If it has ended before
(the relation's `ttl`, with no leg left to leave another), the prefiller's
blocks are gone and the read is invalid: the request's load is 0, its
admission takes its running slot, and it computes those tokens itself, as
vLLM recomputes a read of invalid blocks (`_update_requests_with_invalid_blocks`,
scheduler.py:3140, 3254-3257). PR #719's second review found the first
version reading an expired send in full, so that a finite `ttl` freed the
prefiller's memory at no cost to the decoder. A sweep of `ttl` on
`llmd_nixl_pull.sq` (seed 1, `--horizon 300`, `--blocksD 1000` so that
decoders wait for memory; means):

| `ttl` | D.loaded | D.prefill | ttft | P.kv used |
|---|---|---|---|---|
| none | 5044.8 | 20.1 | 47.80 ms | 1499.1 |
| 0.05 | 4934.4 | 141.4 | 49.89 ms | 1458.0 |
| 0.02 | 4840.7 | 190.9 | 50.86 ms | 1402.0 |
| 0.01 | 4899.3 | 222.0 | 51.93 ms | 1406.1 |
| 0.005 | 4837.2 | 207.7 | 51.04 ms | 1397.2 |
| 0.002 | 4836.4 | 242.6 | 52.21 ms | 1387.0 |

The shorter the `ttl`, the less the prefillers hold, the more the
decoders compute, and the later the first token (not monotone point by
point: each `ttl` is another path of the run). At the example's own
settings a decoder is admitted when the prefill ends and no `ttl` above
0.005 changes anything. The `ttl` is a constant above 0 (`CSend.ttl:
Option<f64>`), `ttl = inf` is refused as the default spelled twice, and
two relations from one source with different `ttl`s do not link.

A read is a load the connector's `load` makes above 0. A decoder preempted
after its read is admitted again, and the examples' connectors load
nothing then (`request.preempted ? 0 : …`; vLLM reschedules it without a
second transfer, `nixl/pull_scheduler.py:187-189`, and prefills locally
what it lost, `scheduler.py:1560-1561`).
That is no load, of a send or of anything, and `E.load_done` keeps its
read's end. v0.1.7-alpha.2 tested for a send gone before reading what the
connector loads, took that readmission for a read of a send gone, and
ended its load at the readmission: `pd_batching.sq` with decoders short of
memory (`--blocksD 300 --Bw 2e4 --x0 0.01`, seed 1, `--horizon 100`)
preempted 89 of 811 decoders after their read, and its `send_held`
overstated their sends' hold (mean 43.6624 s against the log's 43.6491 s);
the companion repository's port found it. The examples' observations of
the read (`admit_wait`, `transfer`, the push's `parked`) read the latest
admission, which for those decoders is not the read's, and are taken only
where it read (`D.loaded > 0`); `send_held` is the send's own and is taken
for every request.

### 5. One spelling of the source

A connector reads a send, which an engine's request registers. A source
queue whose entry holds and leases (`hold … { run P prefill … } lease kv
(inf)`, the kernel's spelling of the same prefill) no longer links as a
connector's source, and the message names `P (prompt, 1);`: its lease was
not the relation's (its `ttl` never reached it). A reader that copies by
hand, an entry's `transfer … from P`, still reads a hold's lease; a `ttl`
on such a relation is refused. No program here uses the refused form;
serving-queue-theory's companion `pd_batching.sq` does, and must move to
the engine form.

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

## Follow-ups

- `at admission` and `mark` are in no program now (the prefillers'
  records say when); removing them is a change of their own
  (`docs/design/prefiller-engine-request.md`). Done: [Two forms no
  program used](unused-header-forms.md).
- serving-queue-theory's `pd_batching.sq` moves to the engine form of the
  prefiller (5).

## Decisions

1. **#715 folds in.** Its prefiller as an engine request stays; its
   `lease` clause does not ship; one IR version carries both.
2. **An unread send ends at the relation's `ttl`**, none by default
   (the session's end, the reports as before); a reader after it
   computes the tokens itself (4).
3. **`join` goes**, refused with what replaced it; a leg calls a source
   engine once and leases nothing, and its send is read on every way
   after it, checked when the program links; a session ending before its
   legs stays a run-time error behind that check.
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

The first step kept one state per session and leg, read by a priority
order from the interpreter's `Status` (where the session's statements
stand) and `Phase` (how far its engine request has gone), and reported
nine states (`waiting`, `preempted`, `loading`, `running`, `transferring`,
`saving`, `growing`, `joining`, `client`). It changed no syntax and no IR,
and every report stayed the same but for its table. It was reverted
because the two enums overlap while a connector's block runs, so its one
state per session was one by a priority order the reader had to learn,
not by construction: a decoder's request was `loading` while its session
was `joining` a leg or queued at a NIC's pool, and a lease outlived the
request whose state it was. Its evidence is the table below, which
showed where a request was in two states at once and which states no
program reached (`growing`, `joining`).

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

