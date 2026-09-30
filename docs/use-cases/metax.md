# vLLM-metax

The latest tag checked on 2026-09-30 is [v0.26.0](https://github.com/MetaX-MACA/vLLM-metax/tree/v0.26.0). Its [README](https://github.com/MetaX-MACA/vLLM-metax/blob/v0.26.0/README.md#L27) states alignment with **upstream vLLM v0.26.0**. The platform/patch paths examined integrate upstream and its runner; they are not evidence that every path uses an unchanged scheduler. Only full attention is considered.

## Tagged behavior

`speculative_decode_perf.py` separates **decode, extend and prefill** regions and sorts decodes by the current step's scheduled-token count. Zero computed tokens identifies prefill; otherwise scheduled-token count and a threshold distinguish decode from extend. Query length here is not total context length. This reorders an already selected runner batch; it is not an admission policy. [Batch reorder][reorder]

The `dbo.py` patch permits CUDA-like platforms in SM control. Its presence does not imply DBO is enabled for every configuration or has a fixed overlap rate. [DBO integration][dbo]

The platform supplies device block-copy and host swap-out methods. Their actual full-attention scheduler/cache use depends on upstream and the complete applied patch set. [Platform][platform]

## Executable seQ approximation

```seq title="examples/vendors/metax.seq"
--8<-- "examples/vendors/metax.seq"
```

This synchronous, non-speculative model uses different cost terms for prefill and decode. Constants are illustrative. Predicting hardware response times requires measured step traces and calibration.

## Remaining gaps

| Requirement | Needed refinement |
|---|---|
| Speculative proposal, verification and acceptance | Separate scheduled, computed and committed progress |
| Query-length regions | Per-request batch tokens/phase and region shapes |
| Region-dependent cost | Closed cost reductions over selected batch layout |
| DBO overlap | Compute/communication resource dependencies and completion |
| KV copy | Source/destination objects pinned until completion |

IR v8 can hold several stages for one flow, so shared link capacity need not be approximated by a faster constant. That does not establish a DBO compute/communication execution graph. Two batches with the same total tokens can have different query-length distributions and acceptance results.

## Oracle scenarios

Compare equal-token batches with different decode query lengths, mixed short-prefill/decode batches, speculative rejection and DBO settings. Observe scheduler selection separately from runner permutation, accepted tokens, KV progress, shapes and release times.

**Validation:** this reduced example links and completes its six requests. Neither patched-upstream equivalence nor device performance has been verified.


[reorder]: https://github.com/MetaX-MACA/vLLM-metax/blob/v0.26.0/vllm_metax/patch/performance/speculative_decode_perf.py#L29
[dbo]: https://github.com/MetaX-MACA/vLLM-metax/blob/v0.26.0/vllm_metax/patch/enhancement/dbo.py#L18
[platform]: https://github.com/MetaX-MACA/vLLM-metax/blob/v0.26.0/vllm_metax/platform.py#L628
