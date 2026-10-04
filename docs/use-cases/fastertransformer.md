# FasterTransformer

FasterTransformer is the first of the four schedulers that Dai, Deng, Li and Peng, *Throughput-Optimal Scheduling Algorithms for LLM Inference and AI Agents* ([arXiv 2504.07347](https://arxiv.org/abs/2504.07347)), compare. It is also the one serQ could not write before `serve only` (#261). This page puts the paper's description, the library's source and the program side by side, and runs the program at the paper's operating point C.

## What the paper says it does

§4 of the paper, verbatim:

> FasterTransformer is a decode-prioritized scheduler without mixed batching. Whenever there are requests in the decoding stage, it batches as many decode tokens as possible (up to the token budget $b_{\max}$) and processes them, leaving requests in the prefill queue untouched. It is *not* work-conserving: prefill tokens wait while the GPU processes decode-only batches.

Table 1 of the paper puts it beside the other three:

| Scheduler | Priority | Batching | serQ |
|---|---|---|---|
| FasterTransformer | decode first | no mixed batching | `serve only (decoders > 0 ? decoding : !decoding);` |
| vanilla vLLM | prefill first | no mixed batching | `serve exclusive prefill;` |
| Orca | prefill first | mixed | `serve by (decoding ? 1 : 0);` |
| Sarathi-Serve | decode first | chunked prefill, mixed | `serve decode first;` |

## What the library does

[NVIDIA/FasterTransformer](https://github.com/NVIDIA/FasterTransformer) is a library, not a server. Its last tag is `release/v5.3_tag`. In the GPT model, a call to `ParallelGpt::forward` takes one batch whose size the caller's tensors fix ([`ParallelGpt.cc#L637`](https://github.com/NVIDIA/FasterTransformer/blob/release/v5.3_tag/src/fastertransformer/models/multi_gpu_gpt/ParallelGpt.cc#L637)). It runs the context (prefill) phase for that batch ([`#L1084`](https://github.com/NVIDIA/FasterTransformer/blob/release/v5.3_tag/src/fastertransformer/models/multi_gpu_gpt/ParallelGpt.cc#L1084)), then the generation loop one step at a time ([`#L1191`](https://github.com/NVIDIA/FasterTransformer/blob/release/v5.3_tag/src/fastertransformer/models/multi_gpu_gpt/ParallelGpt.cc#L1191)). A request cannot join a call that is already running. That is request-level batching: decodes run while a prefill waits for the next call, and no step mixes the two. The paper's rule shares the two properties, decode first and no mixing, but it differs from the library in one way that matters at point C. The paper splits prefills by the token budget $b_{\max}$, and the library prefills a batch's whole prompts in one context phase. The program follows the paper. Results at point C shows what the split costs.

## The program

The deployment is the paper's basic LLM queueing model (§3.1) at operating point C of §6.2, the program of #257 with FasterTransformer's `serve`:

```serq title="examples/single-turn/fastertransformer.sq"
--8<-- "examples/single-turn/fastertransformer.sq"
```

`only` says which residents an iteration serves. Its predicate is read for every resident, from the totals before the iteration (`decoders`) and the resident's own `decoding`. While any request decodes, only decodes are served. A request that is still prefilling stays resident and keeps its slot of `reqs`, but gets no token. With nothing decoding, only prefills are served.

## Results at point C

`serq run`, seed 1, horizon 500 s, 1069 arrivals. Each row changes only the `serve` line, and the last row also sets `bmax`. `pending` is the time average of `holders(reqs) + queued(reqs)`.

| Scheduler | `b_max` | pending mean | pending max | decode batch | batches |
|---|---|---|---|---|---|
| Sarathi (`decode first`) | 128 | 95.3 | 101 | 94.4 | 95 % mixed |
| Orca (prefill first, mixed) | 128 | 174.3 | 269 | 95.0 | 75 % decode only |
| vanilla vLLM (`exclusive prefill`) | 128 | 204.4 | 333 | 95.8 | 0 mixed |
| **FasterTransformer (`only`)** | 128 | **529.4** | **1059** | **1.0** | 99.7 % decode only, 0 mixed |
| FasterTransformer | 1024 | 519.7 | 1037 | 3.3 | 99.3 % decode only, 0 mixed |
| FasterTransformer, whole prompts | 10⁶ | 200.3 | 330 | 89.2 | 0 mixed |

Only Sarathi-128 is stable, as in the paper's Figure 14. The first three rows are #257's numbers, reproduced. The process is deterministic (fixed gaps, fixed lengths), so the seed does not move them. Sarathi-128 sits exactly on the boundary: its token load is $\lambda (290 + 990) / (b_{\max} / t_{b_{\max}}) = 1.000$, and it holds only because nothing varies.

FasterTransformer diverges for the reason the paper gives in §4: "When decode requests have token loads below $b_0$, they are processed sequentially without utilizing the GPU's parallel processing capabilities, while incoming prefill requests remain blocked." The iteration trace (`SERQ_TRACE_ITER=1`) shows how this happens at $b_{\max} = 128$:

- A prompt of 290 tokens is split across iterations of 128.
- The first request whose prefill completes starts to decode, and from then on every other prefill waits, including one already partly prefilled.
- That request decodes its 990 tokens alone, one 46.75 ms iteration per token ($t_1 = c + a$), about 46 s. In that time 99 requests arrive.
- Ten sessions end in 500 s.

At $b_{\max} = 1024$, three or four prefills complete in the same iteration. The decode batch then averages 3.3, and the program is no less unstable.

The last row prefills whole prompts in one iteration, which approximates the library's request-level batching. Then up to `k_max` = 100 requests decode together (89 on average) in each cycle of 54 s: 8.1 s of prefill, then 46.3 s of decode. In that time about 116 requests arrive, so it still diverges, but more slowly. Over horizons of 500, 1000 and 2000 s, pending averages 200, 280 and 432. The split by $b_{\max}$ in the paper's rule turns that slow growth into a batch of one.

## What it checks

- `tests/serve_only.rs` runs the rule on a unit clock. A resident prefill waits until the last decode is done, where `decode first` mixes it in. The opposite rule (prefills alone while one is resident) is written with the same construct. `only` composes with `by`. An excluded admission waits as a resident. A session admitted in the iteration counts among the residents. An engine that excludes every resident waits for the next arrival. The program refuses `only` with `exclusive prefill`, and it refuses a predicate that draws, reads `tokens` or reads `now`.
- `tests/ir.rs` round-trips the example through the IR, and the check gate links and draws it.

## What it leaves out

- **The library's batch boundary.** In FasterTransformer, the caller decides which requests make up a batch, and the batch's prompts are prefilled whole. The whole-prompts row approximates that but does not model the caller. Whether the Triton backend batches this way is not checked here.
- **Memory.** The paper's model has no KV cache, and neither does the program. `k_max = 100` is the only cap.
- **Proof.** That the program is not work-conserving shows in one state of the run. It is not yet a Lean theorem: the Lean fragment serves every resident, and the generator raises `Fragment` on a stage with `only`. #257 tracks that.
