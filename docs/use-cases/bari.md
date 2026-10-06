# RAD: optimal tiling (Bari et al.)

Bari, Hegde, de Veciana, *Optimal Scheduling Algorithms for LLM Inference: Theory and Practice* ([arXiv 2508.01002](https://arxiv.org/abs/2508.01002), POMACS 9(3), SIGMETRICS 2026). This page writes the paper's inference node running its scheduler RAD as a serQ program, states two of its propositions as claims, and proves them in Lean about the program's paths.

## The paper

The paper models a GPU's batch time from how matrix multiplications are tiled. A batch of $n$ tokens passes over $\lceil n/b_{col}\rceil$ output tiles of the linear layers, plus non-linear operations per token and attention terms, its equation (7). A batch whose token count is not a multiple of the tile wastes the rest of the last tile. The paper asks which planner (routing across nodes) and which scheduler (batching on a node) reach the highest request rate the hardware carries.

## The serving system it assumes

- $g$ identical inference nodes behind a resource planner (the program writes one node; Theorem 1's bound is per node);
- batch time (7): $\frac{1}{c_{Lin}}\lceil n/b_{col}\rceil + n/c_{nLin}$ plus attention terms;
- prompts are multiples of the tile, $\mathrm{LCM}(b_{row}, b_{col}, b_{red})$ (Assumption 3);
- enough memory for $b_{col}$ active requests of the largest size (Assumption 4);
- RAD with cycle parameter $N$: Decode Mode when $b_{col}$ requests decode or none is left to prefill (one token for each decoding request), otherwise Prefill Mode (a chunk of $b_{lcm}$ tokens of the oldest prefill).

## The system in serQ

```serq title="examples/papers/bari_rad.sq"
--8<-- "examples/papers/bari_rad.sq"
```

| Paper | serQ |
|---|---|
| tiles $b_{col} = b_{row} = b_{red} = b_{lcm} = 128$ | `let bcol = 128;` |
| batch time (7) without attention | `cost tlin * ceil(tokens / bcol) + tnl * tokens;` (µs) |
| Prefill Mode: one chunk of $b_{lcm}$ of the oldest prefill | `budget bcol; chunk bcol;` with prefills served in admission order |
| Decode Mode iff $\lvert D\rvert = b_{col}$ or $P = \emptyset$ | `serve only (decoders >= bcol \|\| decoders == residents ? decoding : !decoding);` |
| Assumption 3 | `set vp = bcol * floor(~uniform(1, vpmax + 1));` |
| $N = \infty$ | no cycle counter |

A prefill completes only in Prefill Mode, which runs only while fewer than $b_{col}$ requests decode, so $\lvert D\rvert$ never exceeds $b_{col}$. That is Assumption 4's bound on the active requests, without a pool.

## The key propositions

Let $S(t)$ count tokens served before an iteration starting at time $t$.
The program uses $b_{col}=128$, $t_{Lin}=4000$ µs per tile and
$t_{nl}=5$ µs per token, omitting attention costs.

!!! theorem "Theorem 1 — Per-node token-rate bound"
    For any scheduler obeying the batch budget and tiled cost,

    $$S(t)\,(t_{Lin}+t_{nl}b_{col}) \le b_{col}\,t.$$

    Here the capacity bound is $128/4640$ tokens per µs. The claim is a
    pathwise rate bound; divergence above capacity also depends on the
    arrival process.

!!! proposition "Optimal tiling — RAD with no cycle limit"
    If every prompt length is a multiple of 128, each batch is a full
    tile unless every resident is decoding:

    $$\mathrm{tokens}=128
      \quad\text{or}\quad \mathrm{decoders}=\mathrm{residents}.$$

    In the exceptional case, fewer than 128 decoding residents can leave
    the batch partially filled. The program uses $N=\infty$.

## The propositions in serQ

```serq
// Theorem 1
claim token_rate: every iteration of E (served * (tlin + tnl * bcol) <= bcol * now);

// Optimal GeMM tiling, under Assumption 3
claim optimal_tiling given (vp == bcol * floor(vp / bcol)):
  every iteration of E (tokens == bcol || decoders == residents);
```

`given` restricts the claim to workloads whose every request satisfies Assumption 3. The drawn prompt lengths are otherwise any natural numbers in the Lean statement. `decoders == residents` says no resident was left to prefill.

## The proof in Lean

`examples/papers/Bari.lean` proves both generated claims over every path in
their workload families, with up to 500 sessions. Arrival times and output
lengths range over natural numbers; `given` restricts prompt lengths to
whole tiles. These pathwise statements do not assume a Poisson law.

The rate proof bounds each batch's tokens by its cost. The tiling proof
maintains that every remaining prefill is a multiple of 128. In Prefill
Mode, the oldest prefill supplies a full tile; in Decode Mode, the engine
serves one token per decoding resident. A prefill can finish only while
fewer than 128 requests decode, preserving the decoder-count bound.

### Random arrivals

The stochastic proofs use separate Markov kernels. Their request types
have prompt lengths from 128 to 1 024 in whole tiles and output lengths
from 1 to 512. All files below are under `examples/papers/`.

!!! theorem "Theorem 2 — One node with a fixed slot-arrival distribution"
    Let $L$ be the expected number of arriving tokens per slot, with the
    slot's request list drawn from a finite distribution of these types.
    If $L<128$, every reachable state of the job-list chain is positive
    recurrent.

    Proof: `BariRecurrent.lean`, `positive_recurrent`.

A full batch changes backlog by arriving work minus 128 tokens. A partial
batch has fewer than 128 decoding requests and no prefill, so its backlog
is bounded. `BariStable.lean` uses the resulting negative drift to bound
expected hitting times. `BariSim.lean` proves the one-step projection from
the machine to the job list; `BariProgram.lean` bounds returns to an empty
engine on the machine chain.

!!! theorem "Theorem 2 — Uniform random routing across g nodes"
    Suppose $g>0$, each arrival is routed independently and uniformly, and
    all nodes advance one iteration per shared slot. If $L<128g$, each
    node's reachable job-list states are positive recurrent.

    If a slot also has positive probability of bringing **no requests**,
    the joint machine chain returns to all engines empty in bounded
    expected time, uniformly over empty starting machines.

    Proof: `BariNodes.lean`, `positive_recurrent` and `return_idle`.

The extra empty-slot assumption is required for the joint result. Capacity
alone does not imply it when $g>1$. The construction uses synchronised
slots and a finite arrival distribution, not asynchronous Poisson arrivals
across nodes.

For a single node with compound Poisson arrivals, `BariPoisson.lean` proves
bounded expected return time to an empty engine under

$$4640\,\lambda\,\mathbb{E}[v_p+v_d]<128,$$

where $\lambda$ is the arrival rate per µs. This result concerns the
machine chain, not recurrence of every state of a Poisson job-list chain.

## On the run

Run for 100 s with seed 1, at 20 and 30 requests per second:

```bash
serq run examples/papers/bari_rad.sq --horizon 100000000 --warmup 0 --seed 1
serq run examples/papers/bari_rad.sq --horizon 100000000 --warmup 0 --seed 1 --set rate=0.00003
```

Claim-table excerpts, 20 requests/s first:

```
claim           kind             result
token_rate      every iteration  holds (23112 iterations)
optimal_tiling  every iteration  holds (23112 iterations)
```

```
claim           kind             result
token_rate      every iteration  holds (22162 iterations)
optimal_tiling  every iteration  holds (22162 iterations)
```

Theorem 1's bound for this program is $128/4640$ tokens per µs. With a mean of about 760 tokens per request (576 prompt, about 185 output after the cap at 512) it is about 36 requests per second, and both loads are below it: 1 987 of 2 013 and 2 971 of 3 000 requests end within the horizon.

## What it leaves out

- **Machine-state recurrence.** Clocks and ended sessions are retained, so
  complete machine states do not repeat. The results concern job-list
  states or returns to the empty-engine set.
- **Finite cycles and SLAI.** The program uses RAD with $N=\infty$;
  finite-cycle draining and the paper's SLAI latency heuristic are not
  modelled here.
- **Attention.** The program omits attention costs. The Lean fragment
  accepts attention terms only with even integer coefficients.
- **Multiple engines in serQ.** The executable fragment has one engine.
  The multi-node planner and synchronised slots are defined in
  `BariNodes.lean`, not in this `.sq` program.
