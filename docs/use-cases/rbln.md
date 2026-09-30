# vllm-rbln

The latest tag checked on 2026-09-30 is [v0.11.3a21](https://github.com/rebellions-sw/vllm-rbln/tree/v0.11.3a21), an alpha. Its [dependency](https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/pyproject.toml#L41) is **vLLM 0.26.0+cpu**; the plugin's version is separate. Only the native `RBLNScheduler` and `RBLNKVCacheManager` full-attention paths are considered.

## Tagged behavior

`RBLNScheduler.schedule` constrains a batch to decodes or a lone prefill. Accepting a waiting prefill can discard an already selected decode batch. `DecodeBatchBudget` separates a PP-dependent hard cap from a soft cap based on current demand. [Selection][native], [decode budget][budget]

The waiting path calls `allocate_slots` with `full_sequence_must_fit=True`: admission tests whether the current full sequence fits even when the allocation is only for the next chunk. Admission capacity and actual allocation are different quantities. [Scheduler][native]

Sub-block caching finds a prefix inside a larger physical block and creates copy operations into another block. `apply_sub_block_match` transfers source references to the copy operations; `release_copy_ops` releases them. Lookup granularity, allocation granularity and copy lifetime must remain separate. [Cache manager][subblock]

## Compiled inputs and padding

The native runner compiles decode shapes up to the PP-stage ceiling
`max_num_seqs // pipeline_parallel_size`, then rounds active decode requests
up to the smallest configured bucket. Buckets may be linear, exponential or
manual; a missing covering bucket is an error, not an arbitrary new shape.
[Runner bucket configuration](https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/vllm_rbln/v1/worker/rbln_model_runner.py#L479),
[bucket lookup](https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/vllm_rbln/v1/worker/bucketing/bucketing_manager.py#L63)

For text inputs the runner stages a lone prefill with query dimension
`max_num_tokens`, even when its actual chunk is shorter. Decode uses padded
request rows and the step's uniform query length. `InputStager` fills dummy
rows/positions and copies only the actual rectangle. Padding changes device
work and buffers; it must not become extra generated/computed request tokens.
[Input layout](https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/vllm_rbln/v1/worker/rbln_model_runner.py#L1297),
[staging](https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/vllm_rbln/v1/worker/input_stager.py#L71)

`determine_batch_execution_and_padding` requires a uniform query length
(`num_tokens % num_reqs == 0`). Its specialized DP path can force decoding
peers to the top bucket and prefill token dimension when another rank
prefills. This applies even with ordinary one-token full-attention decode;
local phase isolation does not remove cross-rank shape coordination.
[Shape routes](https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/vllm_rbln/v1/worker/dp_utils.py#L135)

For a local non-speculative model, a cost expression can charge fixed
prefill width or bucketed `decoders` while the logical work stays unchanged.
The current example uses illustrative costs and does not emulate these
compiled shapes. See [input shapes](input-shapes.md) for executable cost
expressions and the remaining IR requirements.

## Executable seQ approximation

```seq title="examples/vendors/rbln.seq"
--8<-- "examples/vendors/rbln.seq"
```

`serve exclusive prefill` is the phase-isolation policy. At this PR's base,
it only isolates prefills already resident: admitting a waiting prefill after
selected decodes can still produce a mixed batch. [PR #171](https://github.com/vrvrv/serQ/pull/171)
corrects the existing IR policy to select a lone prefill, cancel displaced
decode work, use the full prefill budget and commit only selected progress.
The correction includes actual iteration-trace regression tests and remains
separate from this documentation PR. `reserve (prompt)` tests the whole prompt's capacity, while the hold allocates only the initial chunk and `growing kv` extends it as computation advances. The live token budget is bound at admission, rather than captured before queuing. There are no prefix hits in this workload.

This is not the full native scheduler, even with the correction. PP caps,
remote-KV decode-ready admission guards and sub-block copy semantics remain
outside the example. Until PR #171 lands, the example also does not guarantee
whole-batch isolation on the base interpreter.

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
