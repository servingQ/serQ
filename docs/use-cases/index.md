# Use cases

How far can serQ describe vendor vLLM scheduling and KV-cache behavior? These pages examine the latest version tags checked on **2026-09-30**, including release candidates and alpha tags. They cover **full attention only** and distinguish the plugin version from its upstream vLLM dependency. Features from development branches or older schedulers are not mixed into the comparison.

The examples run on the current IR v8. They are reduced specifications, not vendor scheduler oracles. Their capacities and cost constants are illustrative, not hardware measurements.

## Version basis

| Repository | Latest version tag | Upstream vLLM basis | Evidence |
|---|---|---|---|
| vLLM | `v0.31.0rc2` (RC) | Same tag | [Tag](https://github.com/vllm-project/vllm/tree/v0.31.0rc2) |
| vllm-ascend | `v0.27.1rc1` (RC) | `v0.27.1` | [Release](https://github.com/vllm-project/vllm-ascend/releases/tag/v0.27.1rc1) |
| vllm-rbln | `v0.11.3a21` (alpha) | `0.26.0+cpu` | [Dependency](https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/pyproject.toml#L41) |
| tpu-inference | `v0.29.0` | Upstream selected by build argument or CI LKG | [Dockerfile](https://github.com/vllm-project/tpu-inference/blob/v0.29.0/docker/Dockerfile#L55), [LKG](https://github.com/vllm-project/tpu-inference/blob/v0.29.0/.buildkite/vllm_lkg.version) |
| vLLM-metax | `v0.26.0` | `v0.26.0` | [README](https://github.com/MetaX-MACA/vLLM-metax/blob/v0.26.0/README.md#L27) |

For tpu-inference, the plugin tag does not establish the installed upstream version: the Docker build argument and CI LKG select it separately. Each source link below uses the selected tag.

## Vendor models

| Repository | Tagged implementation features | Executable serQ model | Remaining refinement |
|---|---|---|---|
| [vLLM](../case-study-vllm.md) | Token budget, prefix lookup, priority/FCFS preemption, deferred free | Existing synchronous engine and request library | Content-key sharing, priority victims, asynchronous scheduling |
| [vllm-ascend](ascend.md) | Short-request classes and aging, job predictors, offload/recompute routing | Static short-before-long admission | Shared policy history, dynamic waiting selection, connector failure |
| [vllm-rbln](rbln.md) | Native phase isolation, PP decode caps, sub-block prefix copy | Whole-batch phase isolation and full-sequence admission gate | PP/remote-KV guards, copy references and cache units |
| [tpu-inference](tpu.md) | Rank schedulers, cache-aware routing, contiguous-first allocation, P/D pipeline | Sequential P/D with a source lease and destination allocation | DP padding/barrier, block placement, cancellation and backlog |
| [vLLM-metax](metax.md) | Runner query-length buckets, DBO patch, block copy | Synchronous non-speculative capacity/cost | Accepted-token progress, bucket shapes and compute/communication dependencies |

MetaX integrates upstream in the paths examined; an OOT repository does not necessarily introduce an independent full-attention scheduler.

## Device input constraints

[Input shapes and padding](input-shapes.md) follows scheduler output into
runner inputs: compiled buckets, token/request padding, uniform query
lengths, dummy metadata and backend/head/block compatibility. Each vendor
page now includes the specific tagged path. Real progress and padded device
work must not be conflated. **Padding/shape IR extensions are deferred**;
the audit is retained as source research, not an implementation commitment.

## Run an example

From the repository root:

```bash
cargo run --release -- check examples/vendors/rbln.sq
cargo run --release -- run examples/vendors/rbln.sq --json
```

Replace `rbln` with `ascend`, `tpu` or `metax`. All four use a finite batch of six requests and observe completed responses. The pages include these files directly, and the regular check gate links and draws them.

## What has been verified

The pages separate tagged-source findings, executable approximations, remaining gaps and proposed oracle scenarios. The examples have been checked and run. Vendor SDKs and hardware were not exercised, and no vendor differential tests were run. The existing [validation corpus](../validation.md) does not validate these vendor tags.

[Vendor requirements for the IR](../design/vendor-ir.md) describes what IR v8 already supplies, what remains missing and how to check the next refinement. It proposes no implemented language changes.
