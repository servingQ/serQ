# Prefill/decode disaggregation over NIXL

`examples/pd-disaggregation/llmd_nixl_pull.sq` models routing, prefill,
KV transfer and decode on separate instances. Its key coupling is a lease:
finished prefill blocks remain allocated until the decoder can receive them.
Decoder memory pressure can therefore retain memory on the prefiller.

The source correspondence uses llm-d `8a2f37d`, its inference scheduler
`13eebdb`, and vLLM `0c87a197` (`ref/vllm`). This is a reduced simulation
model, without a two-scheduler differential oracle or hardware validation.

![llm-d prefill/decode over NIXL as a queueing network](../assets/llmd_nixl_pull.deployment.svg)

The figure is labelled in the words the program is written in
([Engines on devices](../language.md#engines-on-devices)): `pool D.reqs on
D` is the engine's running slots, `pool D.kv on D's device` its memory,
`admitted by D` among its options, and `tokens cap` the tokens of one
iteration.

The router picks a prefill instance and a decode instance, each a box with
its engine, its NIC and its pools, one box per instance there is (`NP` and
`ND`, two each). The read crosses between them, from each prefiller to each
decoder: it holds the prefiller's NIC and the decoder's at once, `P.nic[i]`
and `D.nic[j]`, and moves the prompt's KV from the prefiller's leased
blocks to the decoder's (`P.kv[i] → D.kv[j]`) after the decoder's read
latency.

## Configuration

The example has two prefill instances and two decode instances, block size
16, separate KV and request-slot pools, and a NIC per instance. Costs and
capacities are illustrative; this is not the llm-d guide's gpt-oss-120b
hardware configuration.

## In serQ

One request goes like this. The llm-d router picks a decode pod, the least
busy, and asks its prefix-cache estimate how much of the prompt that pod
already has. A long enough uncached suffix means a remote prefill, for
which it picks a prefill pod: a cache-warm one, then the least loaded, and
one of the equals in turn. A short one means the decode pod prefills the
prompt itself. On a remote
prefill the decode pod's sidecar sends the prompt to the prefiller with
`max_tokens = 1`, waits for its answer, and only then sends the decode
request to its own engine. The prefiller's scheduler frees the request's
slot when the one token is sampled but keeps its blocks leased for the
decoder's read. The decoder's scheduler takes the request when the whole
prompt fits in its blocks and parks it (`WAITING_FOR_REMOTE_KVS`) while its
worker reads the KV from the prefiller. The read done, the prefiller frees
the lease (its full blocks stay cached), and the decoder's request queues
once more for a running slot and a token of budget, recomputes the last
prompt token for its logits, and decodes.

The sections below write that a part at a time: what llm-d or vLLM does,
what the program needs for it, and how serQ says it. The router and the
sidecar are a gateway, each scheduler is its queue's entries, and the two
NICs of a read are links.

### Settings

The constants come before `main` and the arguments open it; the rest of the
program is inside `main`.

```serq title="examples/pd-disaggregation/llmd_nixl_pull.sq"
--8<-- "examples/pd-disaggregation/llmd_nixl_pull.sq:settings"
```

| serQ | llm-d / vLLM | Meaning |
|---|---|---|
| `NP`, `ND` | — | prefill and decode instances |
| `B` | `max_num_batched_tokens` | the token budget of one step |
| `max_seqsP`, `max_seqsD` | `max_num_seqs` | requests resident at once on a prefiller, on a decoder |
| `blocksP`, `blocksD` | `num_gpu_blocks` | KV capacity in blocks on a prefiller, on a decoder |
| `bs` | `block_size` | tokens per KV block |
| `max_model_len` | `max_model_len` | every KV pool holds one request of it: vLLM refuses to start otherwise (`kv_cache_utils.py:864-900`, called at `kv_cache_utils.py:2742`) |
| `a`, `omega`, `beta`, `c0` | — | in seconds: `a` per scheduled token; `omega` per step, to read the weights; `beta` per step per resident context token, to read its KV; `c0` fixed per step |
| `BwP`, `BwD` | — | NIXL bandwidth out of a prefiller and into a decoder, tokens/s (128 KB per token at 25 GB/s); one NIC each, so the tensor-parallel fan-out of the read is not modelled |
| `x0` | — | per-read latency: posting the READ and the step that polls its completion (`nixl/pull_worker.py:545-558`) |
| `thr`, `minp` | prefix-based-pd-decider `nonCachedTokens`, `promptTokens` | when to prefill remotely; `thr = 1` is the guide's always-disagg-pd-decider |

### The router

The router has two profiles. The decode profile comes first: the least busy
decode pod (active-request-scorer). Then the decider: a remote prefill only
when the prompt's uncached suffix on that pod is long enough
(prefix-based-pd-decider: `nonCachedTokens`, `promptTokens`). The prefill
profile then takes a pod that has the prefix (prefix-cache-affinity-filter),
then the least loaded (token-load-scorer), and rotates among pods of equal
score (max-score-picker), so cold prompts spread over idle prefillers
instead of all going to the first. The sidecar sends the prefill
with `max_tokens = 1`, waits for the answer, and then sends the decode
request, its KV read from that prefiller.

In serQ the router is a gateway queue whose `route` is the two `choose`s
and the decider between them:

```serq
--8<-- "examples/pd-disaggregation/llmd_nixl_pull.sq:router"
```

The gateway selects a decoder by `holders + queued`, then compares its
uncached prompt suffix with `thr`. The prefiller is the one with the prefix,
then the least loaded, then a draw: `choose` breaks a tie by the smallest
index, which would send every cold prompt of a lightly loaded deployment to
`P[0]` and leave `P[1]` idle, where llm-d's picker rotates. It runs the
prefill entry before the decode entry. `observe send_held` is how long the prefiller kept its
blocks for the decoder's read, its request's send; a decoder preempted
after its read is admitted again without a second, and its read's end
(`D.load_done`) stands. The router reads the model's actual cache state rather than a
delayed or approximate cache index. The model uses `thr = 1` for
always-remote prefill and larger thresholds to keep short uncached suffixes
on the decoder.

### A prefill instance

A prefiller is vLLM's engine on one device ([the vLLM use case](vllm.md))
with no decode. Its scheduler admits a request with a slot and the blocks
of its first chunk, and room for the whole prompt
(`scheduler_reserve_full_isl`). It computes the prompt in chunks and samples
one token, which is discarded. When that token is sampled the request is
finished on the prefiller: the slot goes and the blocks stay leased for the
decoder's read (`request_finished`: `delay_free_blocks`), 30 s renewed by
the decoder's heartbeats while it waits, so in effect until the read.

The program needs the engine, the two pools, the pod's NIC that the reads
out of it share, and an entry that asks the engine for one token:

```serq
--8<-- "examples/pd-disaggregation/llmd_nixl_pull.sq:prefiller"
```

Nothing in it keeps the blocks: the KV relation below does. `kvt.nixl_pull
(P, D, …)` makes `P`'s requests *sent*: a request ends and the slot goes,
but it registers a send, and its blocks stay until the decoder's connector
has read them, which ends the send (or the relation's `ttl`, `inf` here).
The request is `finished` meanwhile, its blocks kept
([one status per request](../design/request-state.md)). It is the scheduler's
decision, as in vLLM: the NIXL connector's scheduler side returns
`delay_free_blocks` when the request finishes
(`nixl/pull_scheduler.py:248-262`), `_free_request` then skips freeing the
blocks (`scheduler.py:2653-2655`), and the worker only reports them sent
(`scheduler.py:3135-3138`). `P (prompt, 1)` is llm-d's prefill request,
`max_tokens = 1`: the token sampled at the prefill's end is the request's
last, and it decodes nothing.

### A decode instance

A decoder is vLLM's engine on one device with two ways in. Its scheduler
serves its queues in this order: the requests whose KV has arrived first
(vLLM's `skipped_waiting`, `scheduler.py:2383-2385`), then the new ones, and
the first that does not fit stops the step's admissions. The engine's
`connector P` is NIXL: a request whose prefill was remote reads every block
past its local hit from the prefiller it came `from`, once, and a preempted
one loads nothing (`do_remote_prefill` is spent). Its admission is
[`examples/multi-turn/vllm.sq`](vllm.md)'s for a request the pod prefills.
A request that reads its KV (`request.loaded > 0`) is taken when the whole
prompt fits the blocks and a running slot could be had, and takes no slot
while it reads (`WAITING_FOR_REMOTE_KVS`); back in the waiting queue it
takes one (`scheduler.py:1264-1267, 3075-3076`). The `queue D` block stays
open: its two entries, below, are the rest of it.

```serq
--8<-- "examples/pd-disaggregation/llmd_nixl_pull.sq:decoder"
```

`pool kv on D.gpu` says a decoder's request waits for its blocks, not for a
slot, and the decoder's scheduler admits it in an iteration. `nic ps(BwD)`
is the pod's NIC, which the reads into it share.

### Decode with a local prefill

When the router keeps the request local, the decode pod prefills the prompt
itself, as [`examples/multi-turn/vllm.sq`](vllm.md) does. Generation
stops at `max_model_len` tokens (`sched/utils.py:114-120`); the engine
keeps the outputs across a preemption, as vLLM does.

```serq
--8<-- "examples/pd-disaggregation/llmd_nixl_pull.sq:local"
```

### Decode with remote KV

The sidecar's decode request is taken when the whole prompt fits the
blocks, at a step with budget left and a running slot free. It takes
neither, since the request is parked, not running, and it reuses the full
blocks it has of the prompt. In `WAITING_FOR_REMOTE_KVS` every block past
the local hit is read from the prefiller over NIXL, through its NIC and
this pod's at once; the read done, the prefiller's lease ends. There is one
KV transfer per request (`nixl/pull_scheduler.py:187-189`). The last prompt
token is recomputed for its logits. Back in the waiting queue the request
needs a running slot and a token of budget. A request preempted on the
decoder afterwards recomputes what it lost here, as any local prefill would
(`do_remote_prefill` is spent).

In serQ that is a `decode … from P` entry: the same call as the local one,
the request's KV read `from` the prefiller by the engine's connector, whose
admission above reserves a slot without taking one. Its last brace closes
`queue D`:

```serq
--8<-- "examples/pd-disaggregation/llmd_nixl_pull.sq:remote"
```

`reqs (0) reserve (1)` is "a slot must be free, none is taken". The engine
records when the read ends, `D.load_done`, the moment the router's `lease`
observation reads.

### The read

The decoder's worker reads the KV (NIXL pull): it posts the READ and finds
it done at a later step, a fixed wait per read, and the bytes cross the
prefiller's NIC and the decoder's at once. How concurrent reads divide the
two NICs is UCX's and the fabric's, not vLLM's: max-min fairness is the
model, not a measurement ([Bandwidth
sharing](https://github.com/servingQ/serQ/blob/main/docs/design/bandwidth-sharing.md)).
`std/kvt` writes the topology, mode and policy in one line:

```serq
--8<-- "examples/pd-disaggregation/llmd_nixl_pull.sq:transfer"
```

### The client and the server

The workload is not in the program: it is a file of its own,
`examples/workloads/chat_reply.sq`, given when the program runs
(`--workload`), with its own settings: `Lambda` sessions per second, `Z`
the mean tool time, `p` the chance a session continues. Sessions arrive,
each turn brings a prompt of `n` new tokens on a context of `K` and wants
`o` back, and after a turn the session calls a tool and comes back with
probability `p`; the context grows by what the server reports it produced
(`reply.produced`). `server (input, hidden output)` says the
scheduler knows `max_tokens`, not the length (`sched/utils.py:98-119`). Lengths are whole
tokens, as vLLM counts them: a fractional length can leave a request less
than the one token its prefill samples (near `max_model_len`, or preempted
in its last fraction of a token).

```serq title="examples/workloads/chat_reply.sq"
--8<-- "examples/workloads/chat_reply.sq:workload"
```

vLLM refuses a prompt of `max_model_len` tokens or more before it is
scheduled (`input_processor.py:512-536`), so the session ends there:

```serq
--8<-- "examples/pd-disaggregation/llmd_nixl_pull.sq:server"
```

### How evenly the router spreads the decoders

The gauges are read at every instant: the time average of a spread is not
the spread of the per-pod averages. The KV usage is vLLM's `kv_cache_usage`
(`block_pool.py:879-890`, of the free blocks; a cached block nobody holds is
one, `block_pool.py:42-45`) but for its null block, which vLLM takes out of
`num_gpu_blocks` and the pool here does not. The load is the active-request
scorer's.

```serq
--8<-- "examples/pd-disaggregation/llmd_nixl_pull.sq:gauges"
```

??? example "The whole program, and its workload"

    ```serq title="examples/pd-disaggregation/llmd_nixl_pull.sq"
    --8<-- "examples/pd-disaggregation/llmd_nixl_pull.sq"
    ```

    ```serq title="examples/workloads/chat_reply.sq"
    --8<-- "examples/workloads/chat_reply.sq"
    ```

### Scheduler correspondence

| vLLM | serQ | Where |
|---|---|---|
| a prompt of `max_model_len` tokens or more is refused before it is scheduled; a generation stops at `max_model_len` tokens; no KV cache smaller than one request of `max_model_len` is started | `if (input >= max_model_len) { end; }` in `server`; `set produced = min(output, max_model_len - prompt)` in each decode entry; `max_model_len = 16384` below every pool | `input_processor.py:512-536`; `sched/utils.py:114-120`; `kv_cache_utils.py:864-900`, called at `kv_cache_utils.py:2742` |
| the prefiller admits like any vLLM engine: a slot, the blocks of the first chunk, room for the whole prompt, the prefix hit looked up when the scheduler takes the request | `taking reqs (1), kv (min(request.known, request.hit + budget_left(P))) reserve (request.known) reuse (floor(request.known - 1, bs))` in `P`'s engine | the waiting loop, `scheduler.py:868-1128`; [the vLLM use case](vllm.md) |
| the prefiller computes the prompt in chunks and samples one token, which the sidecar discards | `P (prompt, 1);` in `P`'s `prefill` entry: the engine's request for one token, sampled at the prefill's end | `scheduler.py:624-823`; the truncation for Mamba and MTP only, `nixl/base_scheduler.py:409-436` |
| the request finishes on the prefiller: its slot is freed, its blocks are not — `request_finished` returns `delay_free_blocks` and a lease of `kv_lease_duration` (30 s), renewed by the decoder's heartbeats while the request waits | `P`'s request's send, which `kvt.nixl_pull (P, D, …)` makes — the request ends, the slot goes, the blocks stay until the read (`ttl = t` names vLLM's lease; `inf` by default) | `nixl/pull_scheduler.py:191-292`; `_free_request`, `scheduler.py:2628-2657`; the renewal, `nixl/base_scheduler.py:199-238`, `nixl/base_worker.py:3010-3030` |
| the prefiller keeps every computed full block of the prompt cached once the lease ends | `cache (request.input)` in `P`'s `taking`, read when the request ends, applied when the send ends | `_connector_finished`, `scheduler.py:2929-2982`; `kv_cache_manager.py:610-619` |
| the decoder's scheduler looks at its waiting queue only at a step with budget left and a running slot free | `pool reqs on D` and `pool kv on D.gpu`: `D` admits both; `reqs (request.loaded > 0 ? 0 : 1) reserve (1)` — a slot must be free, none is taken while the request loads | `scheduler.py:872-879` |
| the decoder's local prefix hit, then the connector: for a remote prefill every prompt token beyond the local hit is external and loaded asynchronously | the engine's `connector P { load (request.preempted ? 0 : request.input - request.hit); }`, with `reuse (floor(request.known - 1, bs))`; `request.hit` is the local hit; a request `D`'s plain `decode` entry made loads nothing | `scheduler.py:932-954`; `nixl/pull_scheduler.py:34-66` |
| blocks are allocated for the whole prompt, and the request is parked, `WAITING_FOR_REMOTE_KVS`, holding them and no slot; one transfer per request | `kv (request.loaded > 0 ? request.known : …) reserve (request.known)`; `request.preempted` is `do_remote_prefill`, spent | `scheduler.py:1199-1226, 1264-1294`; `nixl/pull_scheduler.py:108-189` |
| the worker reads the blocks from the prefiller over NIXL (pull mode: a NIXL READ issued by the decoder) | the connector's load for a request its entry `decode (prompt) from P` made, and `kvt.nixl_pull (P, D, latency = x0);`: the decoder reads; a fixed wait, then the bytes out of the prefiller's NIC and into the decoder's at once, the two shared max-min fairly (a model, not a measurement: [Bandwidth sharing](https://github.com/servingQ/serQ/blob/main/docs/design/bandwidth-sharing.md)) | `nixl/pull_scheduler.py:168-177`; the worker's `_read_blocks`, `nixl/pull_worker.py:392-575` |
| the read done, the blocks are cached, the last prompt token is marked uncomputed (its logits are needed), and the request is back in the waiting queue, served before new arrivals | after a load the engine computes the last token again and reads the admission again, waiting for the `reqs` it took none of, with `reqs` declared before `kv`; `D.load_done` records when | `_update_waiting_for_remote_kv`, `scheduler.py:3032-3077`; `_try_promote_blocked_waiting_request`, `scheduler.py:3079-3092`; `scheduler.py:2383-2385` |
| the prefiller frees the leased blocks when the read completes | the load ends the send of the prefiller's request the entry took `from P[i]` | `_update_from_kv_xfer_finished`, `scheduler.py:3113-3138` |
| the decoder recomputes the last prompt token and decodes; a request preempted afterwards is rescheduled without a second transfer, prefilling locally what it lost | the engine's request: `D (prompt, produced);` in both entries, its tokens kept across a preemption | `scheduler.py:1560-1561`; `nixl/pull_scheduler.py:187-189` |
| a parked request is in no `running` list and is not preempted; nor is the prefiller's finished one | `preempt lifo` takes the last admitted *resident* of the engine | `scheduler.py:742-813` |
| the decoder keeps the prompt's and the output's full blocks cached | `} cache (prompt + o)` | `kv_cache_manager.py:602-606` |

### Writing xPyD

The deployment is queues: `NP` prefill instances and `ND` decode instances,
each with its own KV pool, request-slot pool, engine and NIC, and one
gateway selected by `gw.route();` in the server.

```
queue gw : gateway { route { … } }                 // the router and the sidecar
queue P[NP] : prefill { device gpu { … kv cap blocksP * bs; } engine on gpu { … } pool reqs on P { … } pool kv on gpu { … } nic ps(BwP); prefill (prompt) { … } }
queue D[ND] : agg, decode  { device gpu { … kv cap blocksD * bs; } engine on gpu { … } pool reqs on D { … } pool kv on D.gpu { … } nic ps(BwD); decode (prompt) { … } decode (prompt) from P { … } }
kvt.nixl_pull (P, D, latency = x0, share = share.maxmin);   // the decoder reads the KV from the prefiller
```

A queue's device, engine and pools are its members': `P[i].kv` is the KV of
the i-th prefiller, `engine on gpu` is the member's engine, named after the
queue (`P`), `pool reqs on P` means the member's own scheduler serves the
pool, and `pool kv on gpu` counts the member's own blocks as its running
requests' memory (`batch.kv_decode`, `batch.kv_prefill`). A decoder's
request waits for its blocks, not for a slot, so its pool is `on D.gpu`:
the decoder's scheduler admits it in an iteration, as vLLM's does. The family's size is the
`let` the router chooses over (`queue D[ND]`, `choose j in ND`), so the
number is written once. Each pod's NIC is its own (`nic ps(BwD);`, the
stage `D.nic`), and `kvt.nixl_pull (P, D, latency = x0, share = share.maxmin);`
from `std/kvt` is the transfer's topology, mode and policy in one line: the decoder reads the KV from the
prefiller and waits `x0` before each read, and `share.maxmin` (from
`std/share`) says concurrent reads divide the two NICs max-min fairly.

The router is the gateway's two `choose`s, and the instance it picked is
the queue the request is handed to: `P[i].prefill (prompt)`, then
`D[j].decode (prompt) from P[i]`. Inside an entry nothing is indexed — `kv`,
`reqs`, `P` are the member's own. `from P[i]` is the pool `P`'s entry
leases, and the read's NICs are the relation's: the source member's
`P.nic[i]` and the reader's own `D.nic[j]`. The decoder's entries call its
engine, `D (prompt, produced);`, and its engine's `taking` and `connector
P` say the admission and the read once for every member, each taking its
own pools: the router's body has no `hold`.

Change `NP` and `ND` in the source to change the instance counts. The
router and queue entries use these family sizes directly.

### Pull and push

The pull example dispatches the prefill and decode entries serially. The
prefiller's request registers a send; the decoder allocates its destination
and its connector reads across both NICs: it runs the copy, loads the
destination's computed KV, and ends the send.

The [push example](https://github.com/servingQ/serQ/blob/main/examples/pd-disaggregation/vllm_nixl_push.sq)
uses `fork { P[i].prefill (prompt); }`, and the decoder's load waits for
the prefill's send before the transfer. Decoder
allocation can proceed while prefill runs, matching the concurrent dispatch
in vLLM's push proxy (`disagg_proxy_pushconnector_demo.py:227-270`). The
write waits for both prefill completion and destination registration
(`nixl/push_scheduler.py:128-205`, `nixl/push_scheduler.py:207-294`). Under
decoder pressure, both models retain the source's blocks until destination
memory is available.

### Limits

- The examples write no `ttl`: a send lasts until it is read or the session
  ends. A relation's `ttl = t` bounds it from the request's end until a
  read begins, after which a late decoder computes the prompt itself;
  vLLM's lease (`nixl/pull_scheduler.py:248-269`) is also renewed by the
  decoder's heartbeats (`nixl/base_worker.py:3010-3030`), which are not
  modeled, and a failed read is recomputed rather than read.
- Transfers do not model bidirectional KV reuse between turns, accelerator
  host buffers or tensor-parallel fan-out.
- Prefix caches are per session, without cross-session content sharing.
- The router reads exact model state; stale or approximate routing metadata
  is not represented.
- Max-min NIC sharing and fixed read latency are modeling assumptions.

## Run the example

```bash
serq run examples/pd-disaggregation/llmd_nixl_pull.sq --workload examples/workloads/chat_reply.sq --horizon 2000 --warmup 200 --seed 1 --json
```

At the defaults (seed 1, horizon 2 000 s, warm-up 200 s), the run gives:

| Metric | Result |
|---|---|
| Remote-prefill fraction | 57.7% |
| Mean TTFT | 27.3 ms |
| Mean response | 69.9 ms |
| Mean source lease | 12.9 ms |
| Rejected / stuck sessions | 0 / 0 |

These are simulation observations, not hardware measurements or intervals
across seeds. The lease includes waiting for decoder capacity and the
transfer. Use `--dump` to inspect per-request observations.

## Validation

The vLLM correspondence is checked against the pinned source by
`scripts/check_citations.py`. `tests/pd_semantics.rs` checks lease lifetime,
release, load, transfer and the coupling between decoder capacity and
prefiller occupancy. These are deterministic tests of the interpreter's
semantics, not differential checks against two real vLLM schedulers.
