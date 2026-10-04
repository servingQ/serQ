# Use cases

Real serving systems and workloads written as serQ programs. Each page puts the source or the traffic next to the program that describes it, says what the program checks, and says what it leaves out.

| Page | What it shows |
|---|---|
| [vLLM](vllm.md) | The vLLM v1 engine, checked against the scheduler oracle |
| [vllm-ascend](ascend.md), [vllm-rbln](rbln.md) | Vendor plugins' scheduling and KV-cache behavior, as reduced specifications |
| [Input shapes and padding](input-shapes.md) | How scheduler output becomes compiled device inputs, per vendor |
| [Different workloads](workloads.md) | One engine under single-turn, chat and subagent traffic |
| [Prefill/decode over NIXL](pd.md) | llm-d's prefill/decode disaggregation with the KV transfer |
| [FasterTransformer](fastertransformer.md) | Decode first, no mixed batching (`serve only`), unstable where Sarathi is stable |
| [Throughput-optimal scheduling](dai.md) | Dai et al.: the engine's capacity, Sarathi's work conservation and FasterTransformer's failure, stated as claims and proved in Lean |
| [RAD: optimal tiling](bari.md) | Bari et al.: the upper bound of Theorem 1 and RAD's full tiles, stated as claims and proved in Lean |
| [Smallest Volume First](kong.md) | Kong et al.: SVF's waiting bound and its competitive ratio 1 + 2/(1 − α), proved in Lean over the program's runs |

The last three pages follow one shape: the paper, its contributions, the serving system it assumes, that system as a serQ program, the paper's propositions, the propositions as `claim`s of the program, and the Lean proof of the claims about every path of the program.

## Vendor plugins

vLLM is the baseline the plugins are compared against. The vendor pages examine the latest version tags checked on **2026-09-30**, including release candidates and alpha tags. They cover **full attention only** and distinguish the plugin version from its upstream vLLM dependency. Features from development branches or older schedulers are not mixed into the comparison.

The examples were verified with IR v9. They are reduced specifications, not vendor scheduler oracles. Their capacities and cost constants are illustrative, not hardware measurements.

### Version basis

| Repository | Latest version tag | Upstream vLLM basis | Evidence |
|---|---|---|---|
| vLLM | `v0.31.0rc2` (RC) | Same tag | [Tag](https://github.com/vllm-project/vllm/tree/v0.31.0rc2) |
| vllm-ascend | `v0.27.1rc1` (RC) | `v0.27.1` | [Release](https://github.com/vllm-project/vllm-ascend/releases/tag/v0.27.1rc1) |
| vllm-rbln | `v0.11.3a21` (alpha) | `0.26.0+cpu` | [Dependency](https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/pyproject.toml#L41) |

### Vendor models

| Repository | Tagged implementation features | Executable serQ model | Remaining refinement |
|---|---|---|---|
| [vLLM](vllm.md) | Token budget, prefix lookup, priority/FCFS preemption, deferred free | Existing synchronous engine and request library | Content-key sharing, priority victims, asynchronous scheduling |
| [vllm-ascend](ascend.md) | Short-request classes and aging, job predictors, offload/recompute routing | FCFS classes with selection-time aging | Shared policy history, priority lanes, connector failure |
| [vllm-rbln](rbln.md) | Native phase isolation, PP decode caps, sub-block prefix copy | Whole-batch phase isolation and full-sequence admission gate | PP/remote-KV guards, copy references and cache units |

### Device input constraints

[Input shapes and padding](input-shapes.md) follows scheduler output into
runner inputs: compiled buckets, token/request padding, uniform query
lengths and dummy metadata. Each vendor
page now includes the specific tagged path. Real progress and padded device
work must not be conflated. **Padding/shape IR extensions are deferred**;
the audit is retained as source research, not an implementation commitment.

### Run an example

From the repository root:

```bash
cargo run --release -- check examples/vendors/rbln.sq
cargo run --release -- run examples/vendors/rbln.sq --json
```

Replace `rbln` with `ascend`. Both use a finite batch of six requests and observe completed responses. The pages include these files directly, and the regular check gate links and draws them.

### What has been verified

The pages separate tagged-source findings, executable approximations, remaining gaps and proposed oracle scenarios. The examples have been checked and run. Vendor SDKs and hardware were not exercised, and no vendor differential tests were run. The existing [validation corpus](../validation.md) does not validate these vendor tags.

Each vendor page records the limitations of its executable example.
The [waiting-selection design](../design/waiting-selection.md) describes the
implemented FCFS class precedence and aging in IR v9. The vendor documentation
adds no language semantics.
