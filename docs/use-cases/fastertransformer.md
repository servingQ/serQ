# FasterTransformer

This example models decode-prioritized scheduling without mixed batching,
as described by Dai et al. in [Throughput-Optimal Scheduling Algorithms for
LLM Inference and AI Agents, §4](https://arxiv.org/html/2504.07347v3#S4).
Whenever a decode is resident, prefills wait even if the decode batch does
not fill the token budget.

## Model and library boundary

The program follows the paper's token-budget model. NVIDIA's library at
`release/v5.3_tag` takes a caller-supplied batch, runs its context phase,
then generates tokens in a loop. The batch is fixed within that call.
See [`ParallelGpt.cc`](https://github.com/NVIDIA/FasterTransformer/blob/release/v5.3_tag/src/fastertransformer/models/multi_gpu_gpt/ParallelGpt.cc):
`batch_size` comes from the output tensor shape, `gpt_context_decoder_->forward`
performs prefill, and the loop over `step_` generates output.

The distinction matters: the serQ example splits prefill by `bmax`, whereas
the library's context phase handles the batch's prompts together. This page
simulates the former and uses a large-budget variant to illustrate the
latter; it does not emulate the library's caller or serving backend.

## The program

`examples/single-turn/fastertransformer.sq` is FasterTransformer's
scheduler as Dai, Deng, Li and Peng model it (§4): decode first, no mixed
batching. The deployment is the paper's basic LLM queueing model (§3.1) at
the operating point C of §6.2. The sections below build it a part at a
time.

### Settings

Point C is CodeLlama-34B on TP1, with batch time
$t_b = 11.28 + 35.47\lceil b/128\rceil$ ms, $k_{max} = 100$, and one request
every 467.5 ms with 290 prefill and 990 decode tokens. The argument opens
`main`, which holds the rest of the program:

```serq title="examples/single-turn/fastertransformer.sq"
--8<-- "examples/single-turn/fastertransformer.sq:settings"
```

| serQ | Paper | Meaning |
|---|---|---|
| `kmax` | $k_{max}$ (§6) | the batch size limit |
| `c`, `a`, `b0` | $t_b$ | seconds per iteration, seconds per step of `b0` tokens |
| `gap` | — | the fixed inter-arrival time (s) |
| `vp`, `vd` | — | prefill and decode tokens per request |
| `bmax` | $b_{max}$ | the token budget, set with `--bmax` |

### At most `k_max` residents

The paper's model has no KV cache, so the only cap is the batch size limit.
In serQ that is a pool of slots, one per request:

```serq
--8<-- "examples/single-turn/fastertransformer.sq:slots"
```

### Decode first, never both

While a request decodes, an iteration batches only decodes and the prefills
wait; with none decoding, it batches only prefills. The program needs a
rule that picks which residents an iteration serves, and in serQ that is a
predicate read by `only` in both halves of the schedule:

```serq
--8<-- "examples/single-turn/fastertransformer.sq:engine"
```

`only` says which residents an iteration serves. Its predicate is read for every resident, from the current resident totals (`running.decoding`) and the resident's own `decoding`. While any request decodes, only decodes are served. A request that is still prefilling stays resident and keeps its slot of `reqs`, but gets no token. With nothing decoding, only prefills are served. The device's `compute` and the engine's `execute` together are $t_b$.

### The workload

Arrivals are deterministic, one every `gap` seconds. Each request holds a
slot of `reqs` while the engine serves it, and `pending` counts the
requests in the system, holding a slot or waiting for one:

```serq
--8<-- "examples/single-turn/fastertransformer.sq:workload"
```

??? example "The whole program"

    ```serq title="examples/single-turn/fastertransformer.sq"
    --8<-- "examples/single-turn/fastertransformer.sq"
    ```

## Results at point C

`serq run`, seed 1, horizon 500 s, 1069 arrivals. Each row writes the listed policy in the engine's `schedule`, with `bmax` overrides shown below. `pending` is the time average of `holders(reqs) + queued(reqs)`.

| Scheduler | `b_max` | pending mean | pending max | decode batch | batches |
|---|---|---|---|---|---|
| Sarathi (`decode first`) | 128 | 95.3 | 101 | 94.4 | 95 % mixed |
| Orca (prefill first, mixed) | 128 | 174.3 | 269 | 95.0 | 75 % decode only |
| vanilla vLLM (`exclusive prefill`) | 128 | 204.4 | 333 | 95.8 | 0 mixed |
| **FasterTransformer (`only`)** | 128 | **529.4** | **1059** | **1.0** | 99.7 % decode only, 0 mixed |
| FasterTransformer | 1024 | 519.7 | 1037 | 3.3 | 99.3 % decode only, 0 mixed |
| FasterTransformer, whole prompts | 10⁶ | 200.3 | 330 | 89.2 | 0 mixed |

The deterministic Sarathi-128 model runs at token load 1:
$1280/0.4675 = 128/t_{128}$ tokens per second. Its bounded queue in this
run should not be read as a stability result for variable arrivals. See
[the Dai example](dai.md) for the formal result and its assumptions.

With FasterTransformer's rule and `bmax = 128`, completing one prefill
starts a decode-only period. That request takes 990 iterations of
$11.28+35.47=46.75$ ms while other prefills wait. Only ten sessions finish
in 500 seconds. At `bmax = 1024`, more prefills finish together, but the
mean decode batch is still only 3.3. The large-budget variant gives much
larger decode batches; it also leaves a substantial backlog in this run.

To reproduce a row, replace the example's `advance running only …; admit
waiting only …` with the listed policy (`advance running decode first;`,
`exclusive prefill;`, … then `admit waiting while (running.preempted ==
0);`) and pass `--bmax …`. These finite runs compare queue accumulation;
they do not by themselves prove stability or divergence.

## Validation

The six table rows were run with the stated settings. `tests/serve_only.rs`
checks subset serving, predicate restrictions and excluded residents;
`tests/ir.rs` checks IR round-trips. No differential comparison with the
FasterTransformer library was performed.

## What it leaves out

- **The library's batch boundary.** In FasterTransformer, the caller decides which requests make up a batch, and the batch's prompts are prefilled whole. The whole-prompts row approximates that but does not model the caller. Whether the Triton backend batches this way is not checked here.
- **Memory.** The paper's model has no KV cache, and neither does the program. `k_max = 100` is the only cap.
- **Proof.** Non-work-conservation is proved for `examples/papers/dai_fastertransformer.sq`, the basic model without `k_max` ([Dai et al.](dai.md)). That theorem does not directly cover this program's resident cap and fractional clock.
