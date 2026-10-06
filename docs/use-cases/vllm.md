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

```serq title="examples/multi-turn/vllm.sq"
--8<-- "examples/multi-turn/vllm.sq"
```

The program shares its request definition with the other existing workloads:

```serq title="lib/vllm.sq"
--8<-- "lib/vllm.sq"
```

| Requirement | Current expression | Limit of the model |
|---|---|---|
| Token budget and chunked prefill | `step { budget …; chunk …; }` | Resident selection then waiting admission is a specific policy |
| Request slots and KV space | `hold`, admission bindings, `reserve`, `growing` | Tagged fit/lookahead conditions are not fully validated |
| FCFS/priority | Queue order and `preempt lifo`; PRIORITY is `preempt by (-priority, -t0) requeue tail` beside `queue by (priority, t0)` | No oracle scenario checks the PRIORITY victim |
| Shared prefix | `cachedin`, `reuse`, `cache` | Cache identity is session-based, not content-key shared objects |
| Local decode preemption | `computed`-aware `known` in `vllm_request` | Latest-tag recovery paths need differential validation |
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
