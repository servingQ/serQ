# vLLM

The executable model and oracle correspondence use vLLM `0c87a197`.
They cover a synchronous scheduling fragment, not every vLLM feature.
The source survey below separately examines full-attention scheduling and
KV cache in [v0.31.0rc2](https://github.com/vllm-project/vllm/tree/v0.31.0rc2).
The oracle does not establish equivalence with that entire version.

## Tagged behavior

`Scheduler.schedule` shares a token budget between resident and waiting requests, using computed progress to determine selected work. Allocation failure triggers victim selection: FCFS and priority paths choose differently. [Scheduler](https://github.com/vllm-project/vllm/blob/v0.31.0rc2/vllm/v1/core/sched/scheduler.py#L570)

`KVCacheManager.get_computed_blocks` finds reusable prefixes, while `allocate_slots` acquires space for scheduled work and lookahead. Identical prefixes can share blocks across requests, requiring more than a session's cached length. [KV manager](https://github.com/vllm-project/vllm/blob/v0.31.0rc2/vllm/v1/core/kv_cache_manager.py#L264)

The scheduler guards whether victim blocks can actually be freed and manages deferred free. `AsyncScheduler` tracks output placeholders and in-flight results. Releasing or publishing KV before completion can create invalid reuse. [Victim/free guard](https://github.com/vllm-project/vllm/blob/v0.31.0rc2/vllm/v1/core/sched/scheduler.py#L773), [async scheduler](https://github.com/vllm-project/vllm/blob/v0.31.0rc2/vllm/v1/core/sched/async_scheduler.py#L14)

## Executable serQ model

![vLLM deployment in serQ](../assets/vllm.deployment.svg)

`examples/multi-turn/vllm.sq` is vLLM v1 on one device at the pinned
revision (`ref/vllm` at 0c87a197): continuous batching with chunked
prefill, a block pool with an LRU prefix cache, and recompute on
preemption. The sections below build it a part at a time. Each says what
vLLM does, what the program needs for it, and how serQ writes it.
[vLLM v1 as a serQ program](../language.md#7-vllm-v1-as-a-serq-program)
has the line-by-line correspondence with citations.

### Settings

vLLM's engine is configured by a handful of numbers. The program names
each once and takes the ones a run varies from the command line
(`serq run examples/multi-turn/vllm.sq --max_seqs 32`). The constants come
before `main` and the arguments open it; the rest of the program is inside
`main`:

```serq title="examples/multi-turn/vllm.sq"
--8<-- "examples/multi-turn/vllm.sq:settings"
```

| serQ | vLLM | Meaning |
|---|---|---|
| `B` | `max_num_batched_tokens` | the token budget of one step |
| `bs` | `block_size` | tokens per KV block |
| `max_seqs` | `max_num_seqs` | requests resident at once |
| `blocks` | `num_gpu_blocks`, less the null block | KV capacity in blocks |
| `chunk_cap` | `long_prefill_token_threshold` | the most one prefill takes per step; `inf` for none |
| `a`, `omega`, `beta`, `c0` | — | the cost model in seconds: `a` per scheduled token; `omega` per step, to read the weights; `beta` per step per resident context token, to read its KV; `c0` fixed per step |
| `Lambda`, `Z`, `p` | — | the workload: sessions per second, mean tool time, the chance a session continues |

### KV memory: a block pool with a prefix cache

vLLM's KV cache is a pool of fixed-size blocks. A finished or preempted
request's full blocks go back to the free queue still hashed, so a later
turn of the same session can reuse them; when the free queue runs dry the
oldest freed block is evicted first. When a running request needs a block
and none is free, the most recently admitted request is preempted: its
blocks go back to the cache, and it re-enters the waiting queue at the
head (`scheduler.py:1539-1582`) to recompute what was lost.

serQ says this with a device that has KV capacity and a pool on it whose
three clauses are those three rules:

```serq
--8<-- "examples/multi-turn/vllm.sq:memory"
```

`block bs` makes the pool hand out whole blocks, `evict lru` evicts cached
blocks tail first, and `preempt lifo` picks the last admitted request as
the victim. The device also carries the cost of a step: `compute` per
scheduled token and `hbm` for the weights plus the KV read.

### The engine's two caps

Each step vLLM schedules at most `max_num_seqs` requests and at most
`max_num_batched_tokens` tokens (`scheduler.py:872, 877-879`). These are the
engine's two capacities, the first lines of its block; the request, the
schedule and the cost below are the rest of it:

```serq
--8<-- "examples/multi-turn/vllm.sq:caps"
```

### What a request does

A request in vLLM is a prompt to prefill and an output to decode, but two
details make it more than that.

- **Preemption keeps the tokens, not the KV.** A preempted request is
  rescheduled with every token it has, the prompt plus what it generated,
  and recomputes their KV (`scheduler.py:1560-1561`). So the program needs
  `known`, every token the request has, read again at each admission:
  `max(kept, computed + 1)`, never fewer than an earlier attempt had.
- **A prefix hit is full blocks, never the last token.** The last token is
  recomputed for its logits (`kv_cache_manager.py:289-300`), so the reusable
  prefix is the cached full blocks of `known − 1`.

In serQ the request is a hold whose `at admission` block computes those two
values at the moment the scheduler admits it:

```serq
--8<-- "examples/multi-turn/vllm.sq:request"
```

`prefill … growing kv` and `decode … growing kv` allocate a block at a time
as the request advances, as `allocate_slots` does. `observe hit` records
whether any block was reused, again after a preemption. `cache (prompt +
output)` leaves a finished request's blocks cached for the next turn.

### Scheduling the running requests

Each step first hands the budget to the running requests in admission
order: one token to each decoding request, and up to the chunk cap to a
prefilling one. vLLM applies `long_prefill_token_threshold` only while
another request is running or waiting; a request alone takes the whole
budget (`scheduler.py:606-616`). That rule is written in the program, not
assumed by the language. It opens the `schedule` block, which the admission
below continues:

```serq
--8<-- "examples/multi-turn/vllm.sq:running"
```

A constant cap would not be vLLM's, and `serq target` refuses it.

### Admitting waiting requests

With the budget the running requests leave, vLLM admits waiting requests
FCFS while the request cap and the KV blocks allow, and the first that does
not fit blocks the rest. Four details shape the admission:

- the prefix lookup happens when the scheduler admits, not when the request
  queues (`scheduler.py:932-939`);
- it allocates the reused tokens plus the chunk the budget allows
  (`scheduler.py:1078-1128, 1214-1226`);
- it admits only if every known token fits, the reused ones counted
  (`scheduler_reserve_full_isl`, on by default: `config/scheduler.py:191`,
  `kv_cache_manager.py:515-531`);
- nothing is admitted in a step that preempted (`scheduler.py:869`).

In serQ these are the `taking` units, the `reserve`, and the `while`, the
last statement of the `schedule` block:

```serq
--8<-- "examples/multi-turn/vllm.sq:waiting"
```

[Admission](../language.md#admission) has the full argument.

### The cost of a step

A step's time is the larger of its compute and its memory traffic, plus a
fixed overhead. It is the engine block's last line:

```serq
--8<-- "examples/multi-turn/vllm.sq:execute"
```

The waiting queue the engine admits from is a pool on the engine, served in
arrival order:

```serq
--8<-- "examples/multi-turn/vllm.sq:queue"
```

### The client

Nothing in the workload is vLLM. Sessions arrive, each turn brings a prompt
of `n` new tokens on a context of `K` and wants `o` back, and after a turn
the session calls a tool and returns with probability `p`. `hidden o` says
the scheduler cannot read the output length: it knows `max_tokens`, not
when EOS comes (`sched/utils.py:98-119`).

```serq
--8<-- "examples/multi-turn/vllm.sq:workload"
```

A turn is one request to the engine, on the whole context:

```serq
--8<-- "examples/multi-turn/vllm.sq:server"
```

[Different workloads](workloads.md) runs this engine under other clients.

??? example "The whole program"

    ```serq title="examples/multi-turn/vllm.sq"
    --8<-- "examples/multi-turn/vllm.sq"
    ```

### What the model leaves out

| Requirement | Current expression | Limit of the model |
|---|---|---|
| Token budget and chunked prefill | `engine … { tokens cap …; scheduler { … each at most (…); } }` | Resident selection then waiting admission is a specific policy |
| Request slots and KV space | `hold`, admission bindings, `reserve`, `growing` | Tagged fit/lookahead conditions are not fully validated |
| FCFS/priority | Queue order and `preempt lifo`; PRIORITY is `preempt by (-priority, -t0) requeue tail` beside `queue by (priority, t0)` | No oracle scenario checks the PRIORITY victim |
| Shared prefix | `cachedin`, `reuse`, `cache` | Cache identity is session-based, not content-key shared objects |
| Local decode preemption | `computed`-aware `known` in the `server` block | Latest-tag recovery paths need differential validation |
| Async and speculative execution | Cost expressions, explicit leases/transfers | In-flight scheduler state and proposed/accepted progress remain absent |

## Validation

[The vLLM correspondence](../language.md#7-vllm-v1-as-a-serq-program) records the existing scheduler scenarios and trace. Those checks apply to their stated reference and paths, not to every feature of v0.31.0rc2.

See [vendor plugins](index.md#vendor-plugins) for the reference versions, executable model and remaining limitations.

## Measured replay

`examples/replay/vllm_replay.sq` models the A100/Qwen3-8B testbed using
`examples/replay/data/short_base.csv` (333 sessions). It uses block size 16,
token budget 512, 64 request slots and 128 160 tokens of KV capacity.
Session `i` starts at `i * spacing`; later turns follow their recorded
think times. This is a separate calibration from the tagged-source survey
above.

### Cost and calibration

`tools/a100/step_fit.json` records a fit to 3 022 measured steps:

```text
cost = c + d * decoders + e * kv_decode + a * prefilled + b * attention
```

| Coefficient | Value |
|---|---|
| `c` | 13.9 ms |
| `d` | 41 µs |
| `e` | 0.138 µs |
| `a` | 51.5 µs |
| `b` | 4.02 ns |

The fit's mean absolute percentage errors are 2.7% for decode, 5.6% for
prefill and 5.7% for mixed steps. For served-path replay, two light-load
runs calibrated `c_it = 4 ms` per step and `c0 = 40 ms` per request.
These are effective parameters: those runs do not uniquely identify the
split between per-step and per-request overhead.

### Reported comparison

| Session spacing | Policy | Predicted mean TTFT | Measured mean TTFT |
|---|---|---|---|
| 3.0 s | Unpinned waiting prefixes | 0.605 s | 0.441 s |
| 2.5 s | Unpinned waiting prefixes | 39.1 s | 34.6 s |
| 2.5 s | Pinned waiting prefixes | 0.888 s | 0.878 s |

The pinning prediction preceded its measurement. Each point is one run;
the two 2.5 s measurements were made on different days, and only the pinned
run carried a step tracer. The run records are not published; the scripts
are in `serving-queue-theory/scripts/exp/`. This table records that
comparison, not a reproducible benchmark shipped with this repository.
