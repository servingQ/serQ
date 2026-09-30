# vllm-ascend

The latest tag checked on 2026-09-30 is [v0.27.1rc1](https://github.com/vllm-project/vllm-ascend/tree/v0.27.1rc1), a release candidate. Its [release notes](https://github.com/vllm-project/vllm-ascend/releases/tag/v0.27.1rc1) specify upstream **vLLM v0.27.1**. This page considers full-attention request/KV paths; the selected scheduler depends on configuration.

## Tagged behavior

`ShortRequestFirstRequestQueue` classifies requests into immediate, short and long queues. Immediate requests take precedence; sufficiently old long requests are promoted ahead of short requests. This is a class-and-aging policy, not simply sorting by length. [Queue implementation][short]

`BatchJobAwareRequestQueue` uses job-level decode-length predictions, available admission budget, cold-start requests and length buckets. A fixed per-request key cannot reproduce its shared history. [Job-aware implementation][job]

In the relevant preemption path, `RecomputeScheduler.schedule` asks the connector to offload. Success proceeds to normal preemption; failure finishes the local request through `_finish_recomputed_request` and returns `stop_reason="recomputed"`. `DyntraLBPolicyMixin` also prefetches remote KV and waits in `WAITING_FOR_REMOTE_KVS`. [Recovery][recompute], [prefetch][dyntra]

## Executable seQ approximation

```seq title="examples/vendors/ascend.seq"
--8<-- "examples/vendors/ascend.seq"
```

The queue key gives short requests precedence and preserves serial order inside each class. The class offset is safe for this six-request workload. Request slots, KV capacity, token budget and chunk size are explicit; prompt and output capacity is allocated upfront to avoid recovery in this example.

## Remaining gaps

| Requirement | Current mechanism | Remaining gap |
|---|---|---|
| Long-request aging | `queue by` at enqueue; `serve by` for residents | Reevaluate waiting classes and select from multiple queues |
| Job prediction and cold start | Session attributes | Shared job history and completion-driven predictor updates |
| Remote KV arrival | `lease`, `load`, `release`, explicit transfer | Connector success/failure, cancellation and readiness protocol |
| Offload or remote recompute | `preempt lifo`, `computed`-aware local recovery | Choose a recovery target and finish/forward to another engine |

IR v8 can retain source memory through a transfer and preserve a local request's known progress on re-admission. Those mechanisms do not by themselves implement Ascend's connector or routing policy.

## Oracle scenarios

Compare a long request crossing the aging threshold under continuous short arrivals; predictor updates after cold start; and offload success, failure and remote recompute during decode. Observe selected requests/tokens, held KV, remote readiness and recovery destination.

**Validation:** this reduced example links and completes its six requests. It has not been compared request by request with the Ascend scheduler or runtime.


[short]: https://github.com/vllm-project/vllm-ascend/blob/v0.27.1rc1/vllm_ascend/core/short_request_first_scheduler.py#L148
[job]: https://github.com/vllm-project/vllm-ascend/blob/v0.27.1rc1/vllm_ascend/core/batch_job_aware_scheduler.py#L372
[recompute]: https://github.com/vllm-project/vllm-ascend/blob/v0.27.1rc1/vllm_ascend/core/recompute_scheduler.py#L415
[dyntra]: https://github.com/vllm-project/vllm-ascend/blob/v0.27.1rc1/vllm_ascend/core/dyntra_lb_scheduler.py#L169
