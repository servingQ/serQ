# vllm-musa

The latest tag checked on 2026-09-30 is [v0.24.0](https://github.com/MooreThreads/vllm-musa/tree/v0.24.0). [third_party/PINS](https://github.com/MooreThreads/vllm-musa/blob/v0.24.0/third_party/PINS#L9) selects **upstream vLLM v0.24.0**. Only full attention is considered.

## Tagged behavior

The platform examined does not select a separate full-attention scheduler; it integrates a pinned upstream and patches. The [upstream scheduler](https://github.com/vllm-project/vllm/blob/v0.24.0/vllm/v1/core/sched/scheduler.py) handles residents, waiting requests, token budget, prefix lookup and KV allocation. This observation does not establish that every conditional patch preserves upstream behavior.

`MUSAPlatformBase.update_block_size_for_backend` selects **64-token blocks** in the supported full-attention path when the user has not specified a size and the backend supports 64. The initial default of 16 need not be the final value. Explicit user settings and other paths can differ. [Block selection](https://github.com/MooreThreads/vllm-musa/blob/v0.24.0/vllm_musa/platform.py#L648)

The platform registers a full-attention FlashAttention backend. Even with unchanged request order, step cost needs calibration for that backend and batch shape. [Backend registration](https://github.com/MooreThreads/vllm-musa/blob/v0.24.0/vllm_musa/platform.py#L107)

## Executable seQ approximation

```seq title="examples/vendors/musa.seq"
--8<-- "examples/vendors/musa.seq"
```

This assumes the backend-selected 64-token path. `cap` counts tokens; `block` rounds physical allocations. Derive capacity from the actual available memory and KV bytes per token. The example allocates prompt/output capacity upfront to avoid preemption and uses illustrative synchronous costs.

## Remaining gaps

| Requirement | Current expression | Remaining refinement |
|---|---|---|
| Final backend block size | `block 64` | Supply the actual configuration value |
| Shared prefix | Same-session cache and reuse | Content-key block sharing and reference lifetime |
| Local decode recovery | `computed` and re-admission; `lib/vllm.seq` | Compare the patched tagged scheduler under pressure |
| Device cost and async lifetime | Token/phase/context cost terms; explicit transfer leases | Calibrated batch shapes and in-flight device completion |

Full attention does not justify a new cache kind here. Correct block configuration and shared-prefix/asynchronous refinement are the immediate requirements.

## Oracle scenarios

Compare explicit versus backend-selected block sizes, reuse across block boundaries, and decode preemption preserving generated output. Observe final block size, selected tokens, physical allocations, hits and recovery progress.

**Validation:** this reduced example links and completes its six requests. No differential test against the patched upstream or hardware performance test has been run.
