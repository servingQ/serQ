# tpu-inference

The latest tag checked on 2026-09-30 is [v0.29.0](https://github.com/vllm-project/tpu-inference/tree/v0.29.0). This plugin version does not establish the installed upstream version: the [Dockerfile](https://github.com/vllm-project/tpu-inference/blob/v0.29.0/docker/Dockerfile#L55) accepts a vLLM checkout build argument, and the [CI LKG](https://github.com/vllm-project/tpu-inference/blob/v0.29.0/.buildkite/vllm_lkg.version) identifies a separate validated upstream snapshot. This page describes the tagged plugin's full-attention paths. DP and P/D are distinct configurations.

## Tagged behavior

`DPScheduler` drives upstream schedulers and logical KV shards per rank, then combines their outputs. Routing supports prefix-hit lookup, load-based selection and round robin. Combined output carries the maximum scheduled-token count across ranks for common padded runner shapes. [DP scheduler][dp]

`ContinuousFreeQueue.popleft_n` prefers a best-fitting contiguous interval for multiple blocks, falls back to scattered allocation if none fits, and takes high IDs first for a single block. `core_tpu.py` injects this queue. Fragmentation changes placement; it does not automatically mean admission fails. [Allocator][continuous], [injection][core]

`_DisaggOrchestrator` runs prefill, transfer and decode using separate threads and a transfer backlog. Source release and destination readiness are part of the pipeline, not just a scalar network delay. [Orchestrator][orchestrator]

## Executable seQ approximation

```seq title="examples/vendors/tpu.seq"
--8<-- "examples/vendors/tpu.seq"
```

IR v8 can express overlapping memory lifetimes directly: P is acquired for prefill and leased when that scope ends; D is acquired afterwards; transfer completion loads D and releases P; D remains held through decode. This avoids reserving D before prefill merely to nest the scopes.

The compute/link work is in seconds. `decode : ps(present)` gives each concurrent request a fixed rate; it is a fluid approximation, not TPU token iterations or DP execution. The example does not claim TPU first-token latency. It has no queue protocol, cancellation or concurrent P/D dispatch.

## Remaining gaps

| Requirement | Available now | Remaining refinement |
|---|---|---|
| Rank KV/compute and routing | Pool/stage families and `choose` | Content-key hits across requests and routing state |
| Combined DP execution | Independent step stages | Joint commit, padding shapes and rank completion barrier |
| Contiguous-first placement | Capacity and block rounding | Block IDs, free intervals and placement-dependent cost |
| P/D memory coupling | Source lease, destination hold, completion-ordered load/release | Backlog protocol, failure/cancellation and dispatch overlap |

The allocator's scattered fallback must be preserved. Equal free capacity with different interval distributions can imply different execution costs without implying different fit results.

## Oracle scenarios

Compare routing with differing rank prefix hits, unbalanced rank padding, equal free-block counts with different intervals, and transfer backlog/cancellation. Observe chosen rank, physical IDs, combined shape, readiness and source/destination release order.

**Validation:** this reduced example links and completes its six requests. Neither TPU scheduler equivalence nor hardware performance has been verified.


[dp]: https://github.com/vllm-project/tpu-inference/blob/v0.29.0/tpu_inference/core/sched/dp_scheduler.py#L428
[continuous]: https://github.com/vllm-project/tpu-inference/blob/v0.29.0/tpu_inference/runner/continuous_block_pool.py#L23
[core]: https://github.com/vllm-project/tpu-inference/blob/v0.29.0/tpu_inference/core/core_tpu.py#L699
[orchestrator]: https://github.com/vllm-project/tpu-inference/blob/v0.29.0/tpu_inference/core/core_tpu.py#L170
