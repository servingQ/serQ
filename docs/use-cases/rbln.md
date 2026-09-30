# vllm-rbln

The latest tag checked on 2026-09-30 is [v0.11.3a21](https://github.com/rebellions-sw/vllm-rbln/tree/v0.11.3a21), an alpha. Its [dependency](https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/pyproject.toml#L41) is **vLLM 0.26.0+cpu**; the plugin's version is separate. Only the native `RBLNScheduler` and `RBLNKVCacheManager` full-attention paths are considered.

## Tagged behavior

`RBLNScheduler.schedule` constrains a batch to decodes or a lone prefill. Accepting a waiting prefill can discard an already selected decode batch. `DecodeBatchBudget` separates a PP-dependent hard cap from a soft cap based on current demand. [Selection][native], [decode budget][budget]

The waiting path calls `allocate_slots` with `full_sequence_must_fit=True`: admission tests whether the current full sequence fits even when the allocation is only for the next chunk. Admission capacity and actual allocation are different quantities. [Scheduler][native]

Sub-block caching finds a prefix inside a larger physical block and creates copy operations into another block. `apply_sub_block_match` transfers source references to the copy operations; `release_copy_ops` releases them. Lookup granularity, allocation granularity and copy lifetime must remain separate. [Cache manager][subblock]

## Executable seQ approximation

```seq title="examples/vendors/rbln.seq"
--8<-- "examples/vendors/rbln.seq"
```

`serve exclusive prefill` isolates the first resident prefill while decodes stall. `reserve (prompt)` tests the whole prompt's capacity, while the hold allocates only the initial chunk and `growing kv` extends it as computation advances. The live token budget is bound at admission, rather than captured before queuing. There are no prefix hits in this workload.

This is not the full native scheduler: seQ serves residents before admitting waiting requests. Its resident-prefill rule cannot replace a decode batch with a newly admitted prefill.

## Remaining gaps

| Requirement | Needed refinement |
|---|---|
| Waiting prefill supersedes selected decodes | Candidate selection/replacement followed by one batch commit |
| PP hard/soft decode caps | Per-request selection and batch constraints, separate from resident slot count |
| Sub-block hit and copy | Independent lookup/allocation units and source/destination objects with copy leases |

Reducing `block` to the sub-block size would also reduce physical allocation. It can match hit counts while predicting the wrong memory pressure.

## Oracle scenarios

Compare waiting prefill arriving during resident decode; PP hard/soft cap divergence; and a partial-block prefix match requiring a copy. Include source eviction and cancellation while copying. Observe final selected batch, computed/committed progress, physical block count and copy reference acquisition/release.

**Validation:** this reduced example links and completes its six requests. No native RBLN differential test has been run.


[native]: https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/vllm_rbln/v1/core/rbln_scheduler.py#L180
[budget]: https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/vllm_rbln/v1/core/utils.py#L178
[subblock]: https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/vllm_rbln/v1/core/rbln_kv_cache_manager.py#L468
