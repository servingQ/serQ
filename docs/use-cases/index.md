# Use cases

Examples of serving deployments, workloads and scheduling policies, with simulation results and formal verification where available.

## Simulation

A real serving system or workload as a program that runs: the source next to
the program, what the runs show, and what the program leaves out.

| Page | What it shows |
|---|---|
| [vLLM](vllm.md) | The vLLM v1 engine, checked against the scheduler oracle |
| [vllm-ascend](ascend.md), [vllm-rbln](rbln.md) | Vendor plugins' scheduling and KV-cache behavior, as reduced specifications ([below](#vendor-plugins)) |
| [Input shapes and padding](input-shapes.md) | How scheduler output becomes compiled device inputs, per vendor |
| [Different workloads](workloads.md) | One engine under single-turn, chat and subagent traffic |
| [Prefill/decode over NIXL](pd.md) | llm-d's prefill/decode disaggregation with the KV transfer |
| [FasterTransformer](fastertransformer.md) | Decode-only batches and their queueing cost (`serve only`) |

## Formal verification

Each example states its model assumptions, expresses properties as program
claims and identifies the Lean proofs. The pages distinguish pathwise
claims from stochastic results and list the assumptions each proof needs.

| Page | What it proves |
|---|---|
| [Throughput-optimal scheduling](dai.md) | Dai et al.: the engine's capacity, Sarathi's work conservation and FasterTransformer's failure |
| [RAD: optimal tiling](bari.md) | Bari et al.: the upper bound of Theorem 1 and RAD's full tiles |
| [Smallest Volume First](kong.md) | Kong et al.: SVF's waiting bound and its competitive ratio 1 + 2/(1 − α) |

## Vendor plugins

The vendor examples model selected **full-attention** scheduling rules.
Their capacities and costs are illustrative. They have been run in serQ,
without vendor SDKs, hardware or differential scheduler tests. The
[vLLM oracle corpus](../language.md#7-vllm-v1-as-a-serq-program) uses a
separate pinned reference and does not validate these plugin versions.

### Version basis

| Repository | Source version | Upstream vLLM basis | Evidence |
|---|---|---|---|
| vLLM | `v0.31.0rc2` (RC) | Same tag | [Tag](https://github.com/vllm-project/vllm/tree/v0.31.0rc2) |
| vllm-ascend | `v0.27.1rc1` (RC) | `v0.27.1` | [Release](https://github.com/vllm-project/vllm-ascend/releases/tag/v0.27.1rc1) |
| vllm-rbln | `v0.11.3a21` (alpha) | `0.26.0+cpu` | [Dependency](https://github.com/rebellions-sw/vllm-rbln/blob/v0.11.3a21/pyproject.toml#L41) |

For compiled buckets and dummy tokens, see [Input shapes and padding](input-shapes.md).
The individual pages describe each model's rules and limitations.

### Run an example

From the repository root:

```bash
cargo run --release -- check examples/vendors/rbln.sq
cargo run --release -- run examples/vendors/rbln.sq --json
```

Replace `rbln` with `ascend`. Both use a finite batch of six requests and observe completed responses. The pages include these files directly, and the regular check gate links and draws them.
