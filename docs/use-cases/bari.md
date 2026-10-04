# RAD: optimal tiling (Bari et al.)

Bari, Hegde, de Veciana, *Optimal Scheduling Algorithms for LLM Inference: Theory and Practice* ([arXiv 2508.01002](https://arxiv.org/abs/2508.01002), POMACS 9(3), SIGMETRICS 2026). This page writes the paper's inference node running its scheduler RAD as a serQ program, states two of its propositions as claims, and proves them in Lean about the program's paths. Issue #260.

## The paper

The paper models a GPU's batch time from how matrix multiplications are tiled. A batch of $n$ tokens passes over $\lceil n/b_{col}\rceil$ output tiles of the linear layers, plus non-linear operations per token and attention terms, its equation (7). A batch whose token count is not a multiple of the tile wastes the rest of the last tile. The paper asks which planner (routing across nodes) and which scheduler (batching on a node) reach the highest request rate the hardware carries.

## Key contributions

- **An upper bound for every planner and scheduler** (Theorem 1): with $\bar d_a$ the expected time a request needs under optimal tiling, a load $\lambda \bar d_a > g$ on $g$ nodes diverges.
- **Two design principles**: *optimal tiling* (batches that fill whole tiles) and *dynamic resource allocation* (splitting a node's time between prefill and decode as the traffic needs).
- **RAD** (Algorithm 1), a scheduler that follows both, and Theorem 2: with a uniformly random planner, RAD is stable (positive-recurrent) below the bound of Theorem 1.
- **SLAI**, a heuristic for latency targets, evaluated on traces.

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

1. **Theorem 1.** Under any planner and scheduler, a node completes at most one token per $t_{Lin}/b_{col} + 1/c_{nLin}$ of time (the paper's $\bar d_a$ per request divides this by the request's tokens), so a load above it diverges.
2. **Optimal tiling (§4.2).** With prompts a multiple of the tile (Assumption 3), every batch RAD schedules fills its tile, unless every request at the node decodes and fewer than $b_{col}$ of them do.

## The propositions in serQ

```serq
// Theorem 1
claim token_rate: every iteration of E (served * (tlin + tnl * bcol) <= bcol * now);

// Optimal GeMM tiling, under Assumption 3
claim optimal_tiling given (vp == bcol * floor(vp / bcol)):
  every iteration of E (tokens == bcol || decoders == residents);
```

`given` restricts the claim to workloads whose every request satisfies Assumption 3. The drawn prompt lengths are otherwise any natural numbers in the Lean statement. `decoders == residents` says the batch decoded every request at the node: no request was left to prefill.

## The proof in Lean

The generated statement (`lean/Serq/Claims.lean`):

```lean
def family_optimal_tiling (w : Workload) : Prop :=
    w.init.length ≤ 500 ∧ w.turns = [] ∧ w.turnSlot = none ∧ w.computedSlot = some 8 ∧
    ∀ i < w.init.length, (((w.attr i 10) = (128 * ((w.attr i 10) / 128))))

def optimal_tiling : Prop :=
  EveryIteration deployment family_optimal_tiling prog fun r => ((r.stats.tokens = 128) ∨ (r.stats.decoders = r.residents))
```

The arrival times are any natural numbers (the program's `poisson`), and the output lengths any. The proofs are in `lean/Serq/Papers/Bari.lean`.

- **`token_rate`** is `Exec.served_rate` (as for Dai et al.), with `tiled_rate`: a batch of $b \le b_{col}$ tokens lasts $t_{Lin}\lceil b/b_{col}\rceil + t_{nl}\,b \ge b\,(t_{Lin}/b_{col} + t_{nl})$. Attention terms only lengthen a batch, so the bound holds with them too.
- **`optimal_tiling`** uses `every_iteration_of` with an invariant `R` of the run. Every job at the engine has work left. A prefill's work is a multiple of 128: it starts at `vp`, and the program never sets `vp` (`RadOK`, read from the program through `Serq/Inv.lean`'s sub-program invariant). The jobs' owners are distinct and at the engine. An entry of the running batch is a full tile or a decode's token. The invariant is checked for each command a session can run (`rad_exec`), for settling an instant (`rad_settle`), and for the end of an iteration (`rad_handle`). There a prefill loses exactly one tile, so its work stays a multiple of 128. At an iteration's start, `assign_eq_fillIter_only` (`Serq/Work.lean`) says the batch is the greedy fill of the residents `serve only` admits, and `rad_batch` splits on the mode. In Decode Mode the batch is one token for each of up to 128 decodes: 128, or fewer when every resident decodes. In Prefill Mode it is one chunk of the oldest prefill, whose work is at least 128.

**Theorem 2 for random arrivals** (`lean/Serq/Papers/BariStable.lean`) is a theorem on the program's Markov kernel (`Serq/Chain.lean`, [the design](../design/stability.md)), not a claim. In each slot, one iteration of the engine, the arrivals are a list of (prompt, output) lengths drawn from a finite distribution, prompts whole tiles of at most 1 024 and outputs at most 512 (`Fits`), as the program draws them. The backlog is the left work of every job plus the output a prefilling request will still decode. RAD's batch is full unless every resident decodes and fewer than 128 do, which is `optimal_tiling` again, now an invariant of the kernel's states. So:

- a slot with a full batch changes the backlog by exactly the arrivals' tokens minus 128 (`backlog_slot`);
- a state whose batch is not full, or whose engine is idle, has backlog below $128 \cdot 512$ (`backlog_lt_of_F`);
- below capacity, mean tokens per slot below 128, the backlog drifts down by $\varepsilon = 128 - \text{load}$ outside that set (`drift`), and Foster's criterion bounds the expected number of slots to reach it by $\text{backlog}/\varepsilon$ (`hitTime_le`, finite by `hit_tendsto`), with a finite expected return time (`returnTime_le`).

A full batch lasts $t_{Lin} + 128\,t_{nl} = 4640$ µs whatever its mode, so the load per slot is the paper's $\lambda \bar d$ per 4 640 µs, and the condition is Theorem 1's bound. Two examples check the theorem is not vacuous.

## On the run

`serq run`, seed 1, 100 s, at 20 and 30 requests per second:

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

- **Positive recurrence in the textbook sense, and Poisson in continuous time.** `BariStable` proves finite expected return times to a set of bounded backlog on the program's kernel, with arrivals drawn at each iteration (at most 10 000), as for Dai et al. The same gaps remain: the states keep the clock and the ended sessions, so the chain is not irreducible as it stands, and the step from finite return times to positive recurrence is not proved (`docs/design/stability.md`).
- **The random planner over $g$ nodes.** The kernel is one node; Theorem 2's planner splits a Poisson stream uniformly, which gives each node a thinned stream of the same kind.
- **The cycle parameter $N$.** The program takes $N = \infty$. A finite $N$ ends a cycle by finishing the active requests, with batches that may not fill a tile, and the paper's tiling principle excepts them in the same way.
- **Attention.** The cost omits (7)'s attention terms, whose coefficients are not integers in µs. The Lean fragment reads `attention` with even integer coefficients only.
- **Several nodes.** The fragment has one engine. Theorem 1 for $g$ nodes is the sum of the per-node bound.
