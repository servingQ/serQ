# Smallest Volume First (Kong et al.)

Kong, Qi, Ye, Zhou, *Geometry-Aware Online Scheduling for LLM Serving: From Theoretical Bound to System Practice* ([arXiv 2606.22327](https://arxiv.org/abs/2606.22327), v2, June 2026). This page writes the paper's memory-constrained serving model with its scheduler SVF as a serQ program, states the core of its competitive-ratio theorem as a claim, and proves the claim and the theorem in Lean about the program's paths.

## The paper

The paper schedules requests whose KV cache grows while they decode. Request $i$ has a prompt of $s_i$ tokens and an output of $o_i$ tokens, holds $s_i + t$ tokens of memory after $t$ decode steps, and decodes one token per step. The GPU has memory $M$. The scheduler chooses which waiting request to start, without preemption. The objective is the total end-to-end latency (TEL). The paper compares a scheduler's TEL with the best schedule in hindsight (OPT).

## The serving system it assumes

The model of §3:

- discrete time; one decode token per request per step;
- memory $M$; an active request $i$ that started at $k$ holds $s_i + t - k$ at step $t$, $k < t \le k + o_i$ (constraint (c2));
- no preemption; admission must preserve the memory constraint;
- burst arrivals for Theorem 3.2: every request arrives at time 0.

## The system in serQ

```serq title="examples/papers/kong_svf.sq"
--8<-- "examples/papers/kong_svf.sq"
```

| Paper | serQ |
|---|---|
| memory $M$ | `pool kv { cap M; ... }` |
| SVF: admit the least volume, ties in arrival order | `queue by (s * o + floor((o * o + o) / 2));` |
| a request holds its peak $s + o$ from admission | `hold kv (s + o) { ... }` |
| one decode token per step, every active request | `budget 1000000; cost 1;` and `run engine decode (o);` |
| burst arrivals | `arrive batch(N);` |
| $\mathrm{vol}_i$, $o_i$, TEL | `observe vol`, `observe out`, `observe latency = now;` |
| $\alpha = P/M$ | `let P = 2500;` and `given (s + o <= P ...)` |

The program admits a request when its peak fits next to the active requests' *peaks*. The paper's forward-looking check admits it when the active requests' future memory leaves room for its peak, which never refuses more. The proof of Theorem 3.2 uses one fact about admission: while request $j$ waits, the active peaks exceed $(1-\alpha)M$. Both rules give it. With the peak held from admission, the memory the program reports is the peaks' sum, not the trapezoid's.

## The key propositions

For the burst-arrival model above, let $W_j$ be request $j$'s waiting time,
$p_i = s_i + o_i$ its peak memory, and
$\mathrm{vol}_i = s_i o_i + (o_i^2 + o_i)/2$ its volume.
Write $i \prec j$ for the order in which SVF admits requests and $(i)$ for
ascending volume order. Assume $M>0$, $o_i\ge1$ and $p_i \le \alpha M$ with $0 \le \alpha < 1$.

!!! proposition "Lemma A.2 — Waiting-time bound"
    While request $j$ waits, active peaks exceed $(1-\alpha)M$.
    Each preceding request holds its peak for $o_i$ steps, with
    $p_i o_i < 2\,\mathrm{vol}_i$. Therefore

    $$
    (1-\alpha)M \sum_j W_j
    \le 2 \sum_j \sum_{i \prec j} \mathrm{vol}_i.
    $$

!!! proposition "Proposition 3.1 — Lower bound on total latency"
    Every schedule feasible under the paper's memory constraint satisfies

    $$
    M\,\mathrm{TEL}
    \ge \sum_j \sum_{i \le j} \mathrm{vol}_{(i)}.
    $$

!!! theorem "Theorem 3.2 — SVF competitive ratio"
    When all requests arrive at time zero, SVF satisfies

    $$
    \mathrm{TEL}(\mathrm{SVF})
    \le \left(1 + \frac{2}{1-\alpha}\right)\mathrm{TEL}(\mathrm{OPT}).
    $$

The waiting-time bound is a program claim proved in `examples/papers/Kong.lean`.
The lower bound is in `examples/papers/KongMath.lean`; `competitive_ratio`
in `examples/papers/Kong.lean` applies it to the program; the [Lean proof](#the-proof-in-lean) below
explains how they compose.

## The propositions in serQ

The first is a claim of the program, about its own runs:

```serq
claim queueing_bound given (s + o <= P && o >= 1):
  at end ((M - P) * (total(latency) - total(out)) <= 2 * (prefix_total(vol) - total(vol)));
```

`total(latency) - total(out)` is $\sum_j W_j$. Every request arrives at 0, so its latency is its waiting time plus its $o_j$ steps. `prefix_total(vol)` is $\sum_k \sum_{i \le k} \mathrm{vol}_{(i)}$, the volumes sorted ascending, and `prefix_total(vol) - total(vol)` is $\sum_j \sum_{i \prec j} \mathrm{vol}_i$. `given` is the paper's hypothesis $p_i \le \alpha M$, with $o \ge 1$. The second and third propositions are about OPT, a schedule no program runs, so they are Lean theorems that take the claim as their premise.

## The proof in Lean

`examples/papers/Kong.lean` proves `queueing_bound` for every terminating
path in the generated workload family: up to 500 requests, memory
$M=20\,000$, peak limit $P=2\,500$ and $o_i\ge1$. The claim is conditional
on all sessions ending; it is not itself a termination theorem.

The proof tracks each request's remaining decode work. While request $j$
waits, active peaks sum to at least $M-P+1$. Each decode step reduces the
remaining memory-time ahead of $j$ by that amount. At admission this gives
a bound on $W_j$; summing the bounds and using $p_i o_i<2\,\mathrm{vol}_i$
gives the program claim.

`KongMath.lean` proves the lower bounds on every feasible comparison
schedule $\sigma$. `Kong.lean` combines them with the claim to obtain

$$(M-P)\,\mathrm{TEL}(\mathrm{SVF})
  \le (3M-P)\,\mathrm{TEL}(\sigma).$$

Thus `competitive_ratio` covers every terminating program path in the
family and every schedule feasible under the comparison model.

### Poisson results

`examples/papers/KongPoisson.lean` proves conditional versions of Theorems
3.4 and 3.5 for Poisson arrivals and geometric output lengths. They are
ratios of expected total latency, not expectations of a per-run ratio.
The proofs assume four steady-state relations:

- the per-class waiting-time balance at rate $(1-\alpha)M/2$;
- the residual-work identity $W_0=\mathbb{E}[U_{run}]$;
- the lower bound on expected OPT;
- expected SVF latency bounded by expected decode time plus wait.

These premises are not derived from the program's chain. In particular,
the peak bound needed for the waiting-time balance is incompatible with
unbounded geometric output lengths unless further restrictions are added.
The 1-bit result also requires $\mathbb{E}[v]>\epsilon$, where $\epsilon$
is the proxy-volume penalty defined in the proof. The burst-arrival
program above does not establish either Poisson result.

## On the run

Run the default 200-request burst with seed 1:

```bash
serq run examples/papers/kong_svf.sq --instance examples/papers/instances/kong_svf/default.sq
```

Selected report columns ($M=20\,000$, $P=2\,500$):

```
observe  count         mean    cv2          p99
out        200     252.8300  0.324     496.0000
vol        200  304613.7000  0.645  895093.0000
latency    200    1144.9800  0.860    3760.0000

claim           kind    result
queueing_bound  at end  holds
```

All 200 requests finish and the claim holds on this run. With
$\alpha=P/M=0.125$, the proved competitive bound is approximately 3.29.
This burst is not a stationary latency sample; the excerpt omits the
report's confidence intervals.

## What it leaves out

- **Growing memory.** The program reserves each request's peak from
  admission. Reported memory is the sum of reserved peaks, not the sum of
  growing KV footprints; the admission policy can be more conservative
  than the paper's forward-looking check.
- **1-bit scheduling.** The program uses exact volume. It does not model
  the proxy queue key or establish Theorem 3.3's bound.
- **Prefill time.** Prompts consume memory but no processing time. Adding
  prefill would change the hypotheses of these results.
