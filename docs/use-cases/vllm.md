# vLLM

The latest version tag checked on 2026-09-30 is [v0.31.0rc2](https://github.com/vllm-project/vllm/tree/v0.31.0rc2), a release candidate. This page considers **full-attention request scheduling and KV cache** in that tag. The existing executable program is a restricted synchronous model; its historical oracle results do not establish equivalence with the entire latest scheduler.

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
| Request slots and KV space | `hold`, admission bindings, `reserve`, `growing` | Match all tagged fit/lookahead conditions with an oracle |
| FCFS/priority | Queue order and `preempt lifo` | Priority victim selection is not the LIFO mechanism |
| Shared prefix | `cachedin`, `reuse`, `cache` | Cache identity is session-based, not content-key shared objects |
| Local decode preemption | `computed`-aware `known` in `vllm_request` | Latest-tag recovery paths need differential validation |
| Async and speculative execution | Cost expressions, explicit leases/transfers | In-flight scheduler state and proposed/accepted progress remain absent |

## Validation and next scenarios

[How serQ is checked](../validation.md) records the existing scheduler scenarios and trace. Those checks apply to their stated reference and paths, not to every feature of v0.31.0rc2. Current local recovery already accounts for known generated progress; the older [IR v4 discussion](../design/ir-v4.md) is a historical design record, not a list of current missing features.

For the latest tag, compare priority victims, output-preserving decode preemption, cross-request prefix sharing and deferred free of in-flight blocks. Observe per-step selection, physical allocation/reuse, committed tokens and release times.

See [vendor plugins](index.md#vendor-plugins) for each repository's latest tag, executable model and remaining limitations.
