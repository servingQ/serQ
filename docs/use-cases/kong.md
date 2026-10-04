# Smallest Volume First (Kong et al.)

Kong, Qi, Ye, Zhou, *Geometry-Aware Online Scheduling for LLM Serving: From Theoretical Bound to System Practice* ([arXiv 2606.22327](https://arxiv.org/abs/2606.22327), v2, June 2026). This page writes the paper's memory-constrained serving model with its scheduler SVF as a serQ program, states the core of its competitive-ratio theorem as a claim, and proves the claim and the theorem in Lean about the program's paths. Issue #258.

## The paper

The paper schedules requests whose KV cache grows while they decode. Request $i$ has a prompt of $s_i$ tokens and an output of $o_i$ tokens, holds $s_i + t$ tokens of memory after $t$ decode steps, and decodes one token per step. The GPU has memory $M$. The scheduler chooses which waiting request to start, without preemption. The objective is the total end-to-end latency (TEL). The paper compares a scheduler's TEL with the best schedule in hindsight (OPT).

## Key contributions

- **A volume view of a request**: its memory-time, $\mathrm{vol}_i = s_i o_i + (o_i^2 + o_i)/2$, the area of the trapezoid its KV cache draws over its life.
- **Smallest Volume First (SVF)**: admit the waiting request of least volume while its peak fits.
- **A lower bound on OPT** (Proposition 3.1): $\mathrm{TEL}(\mathrm{OPT}) \ge \frac1M \sum_j \sum_{i \le j} \mathrm{vol}_{(i)}$, the volumes sorted ascending.
- **A constant competitive ratio** (Theorem 3.2): with every peak $p_i = s_i + o_i \le \alpha M$ and all requests arriving at once, $\mathrm{TEL}(\mathrm{SVF}) \le (1 + \frac{2}{1-\alpha})\,\mathrm{TEL}(\mathrm{OPT})$: at most 3 as $\alpha \to 0$, where the prior best was 48.
- A 1-bit variant (Theorem 3.3), Poisson bounds (3.4, 3.5), and an implementation in vLLM.

## The serving system it assumes

The model of §3:

- discrete time; one decode token per request per step;
- memory $M$; an active request $i$ that started at $k$ holds $s_i + t - k$ at step $t$, $k < t \le k + o_i$ (constraint (c2));
- no preemption; a request is admitted when it fits with the active requests' peaks (the "forward-looking memory check" of the proof);
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

The program admits a request when its peak fits next to the active requests' *peaks*. The paper's forward-looking check admits it when the active requests' future memory leaves room for its peak, which never refuses more. The proof of Theorem 3.2 uses one fact about admission: while request $j$ waits, the active peaks exceed $(1-\alpha)M$. Both rules give it. With the peak held from admission, the memory the program reports is the peaks' sum, not the trapezoid's. Holding $s$ and growing to $s+o$ under a promise of the peak is #262's `commit`.

## The key propositions

1. **Lemma A.2 (the waiting time).** While request $j$ waits, the active peaks exceed $(1-\alpha)M$. The active requests precede $j$ in volume order, and each holds its peak $o_i$ steps, with $p_i o_i < 2\,\mathrm{vol}_i$. Summed over the requests: $(1-\alpha)M \sum_j W_j \le 2 \sum_j \sum_{i \prec j} \mathrm{vol}_i$.
2. **Proposition 3.1.** Every feasible schedule has $M\cdot\mathrm{TEL} \ge \sum_j \sum_{i \le j} \mathrm{vol}_{(i)}$.
3. **Theorem 3.2.** $\mathrm{TEL}(\mathrm{SVF}) \le (1 + \frac{2}{1-\alpha})\,\mathrm{TEL}(\mathrm{OPT})$.

## The propositions in serQ

The first is a claim of the program, about its own runs:

```serq
claim queueing_bound given (s + o <= P && o >= 1):
  at end ((M - P) * (total(latency) - total(out)) <= 2 * (prefix_total(vol) - total(vol)));
```

`total(latency) - total(out)` is $\sum_j W_j$. Every request arrives at 0, so its latency is its waiting time plus its $o_j$ steps. `prefix_total(vol)` is $\sum_k \sum_{i \le k} \mathrm{vol}_{(i)}$, the volumes sorted ascending, and `prefix_total(vol) - total(vol)` is $\sum_j \sum_{i \prec j} \mathrm{vol}_i$. `given` is the paper's hypothesis $p_i \le \alpha M$, with $o \ge 1$. The second and third propositions are about OPT, a schedule no program runs, so they are Lean theorems that take the claim as their premise.

## The proof in Lean

The generated statement (`lean/Serq/Claims.lean`):

```lean
def queueing_bound : Prop :=
  AtEnd deployment family_queueing_bound prog fun m =>
    ((17500 * ((Exec.total m 2) - (Exec.total m 0))) ≤ (2 * ((Exec.prefixTotal (Exec.values m 1)) - (Exec.total m 1))))
```

For every number of requests up to 500, every $(s_i, o_i)$ with $s_i + o_i \le 2500$ and $o_i \ge 1$, and every machine of every path at which every session has ended, the inequality holds. The proof is `lean/Serq/Papers/Kong.lean`. It is the paper's proof, made an invariant of the executable semantics.

**A ghost state.** Each request has a place: before its first command, waiting, admitted, decoding, finished, released, ended. A ghost also keeps the decode work left and the observed latency. `SInv` relates the ghost to the machine: each session's program, stack and status in its place, the pool's queue and holders, the jobs, the ready list, and the observations as a permutation of the expected ones.

**The potential.** For every waiting request $j$:

$$17501 \cdot \mathit{now} + \sum_{i \prec j} p_i \cdot \mathit{rem}_i \;\le\; \sum_{i \prec j} p_i\, o_i,$$

with $\mathit{rem}_i$ the work request $i$ has left ($o_i$ while waiting). When $j$ is admitted it keeps the certificate $17501 \cdot W_j \le \sum_{i \prec j} p_i o_i$. That is Lemma A.2 with the strict inequality of natural numbers, $M - P + 1$ for $(1-\alpha)M$.

**The steps.** Each operation of the semantics keeps `SInv`:

- the first commands of a request (observe its length and volume, queue): `sinv_pop_s0`;
- an admission, which takes the least volume, ties by serial number (`argminKey_least`), its potential becoming its certificate: `sinv_admit`;
- the start of a decode: `sinv_pop_r2`;
- a finished request releasing its peak, the pool admitting what now fits, the latency observed: `sinv_pop_r1`.

`settle` runs these until nothing is ready. Two measures show that the fuel of `drain` and `settleLoop` suffices for 500 requests, and the last admission round leaves the head of the queue not fitting: `sinv_settleLoop`. So at every boundary, while anyone waits, the active peaks are at least $M - P + 1 = 17501$ (`Settled.full`). Every decoding request gets one token, and the iteration lasts one step (`start_bnd`).

**The iteration's end** (`end_sinv`) is where the inequality is earned. Every decoding request advances one token, so each waiting $j$'s sum drops by the active peaks: all of them precede $j$, and they exceed 17501, while $\mathit{now}$ grows by 1.

**The end.** `reach_bnd` gives the invariant at every machine of every path. When every session has ended, the certificates are $17501\,(\mathit{lat}_j - o_j) \le \sum_{i \prec j} p_i o_i$. With $p_i o_i + o_i \le 2\,\mathrm{vol}_i$ (`vol_bound`) and $\sum_j \sum_{i \prec j} \mathrm{vol}_i = \sum_{\text{pairs}} \min = \mathtt{prefix\_total} - \mathtt{total}$ (`sum_prec`, `prefixTotal_eq`), the claim follows (`queueing_bound`).

**Theorem 3.2** (`competitive_ratio`). `lean/Serq/Papers/KongMath.lean` proves Proposition 3.1 for every schedule feasible in the paper's model (start times $x_i$, memory $s_i + t - x_i$ per active request, at most $M$ at every step): `opt_lower_bound`. It also proves the composition: from the claim, $M\cdot\mathrm{TEL}(\sigma) \ge \mathtt{prefix\_total}$ and $\mathrm{TEL}(\sigma) \ge \sum o$,

$$(M - P)\,\mathrm{TEL}(\mathrm{SVF}) \;\le\; (3M - P)\,\mathrm{TEL}(\sigma),$$

that is $\mathrm{CR} \le \frac{3M-P}{M-P} = 1 + \frac{2}{1-\alpha}$, for every path of the program and every feasible $\sigma$.

## On the run

`serq run`, seed 1, 200 requests, $M = 20000$, $P = 2500$ ($\alpha = 0.125$, bound $1 + 2/0.875 = 3.29$):

```
observe  count         mean    cv2          p99
out        200     252.8300  0.324     496.0000
vol        200  304613.7000  0.645  895093.0000
latency    200    1144.9800  0.860    3760.0000

claim           kind    result
queueing_bound  at end  holds
```

The report's confidence intervals are left out: the 200 requests arrive together and are served in volume order, so their latencies are not independent. On this run the claim holds with room: its left side is 46 % of its right side, and TEL(SVF) is 1.33 times the larger of the two lower bounds on OPT, against the bound 3.29.

## What it leaves out

- **The trapezoid.** The program holds the peak from admission. The memory over time is then $p_i(o_i+1)$, not $\mathrm{vol}_i$. The admission rule and the theorem are the paper's, and #262 would let a hold promise its peak while it grows.
- **Theorem 3.3 (1-bit SVF)** is the same program with a proxy key (`queue by` on the class). Its $O(T)$ bound is not claimed.
- **Theorems 3.4 and 3.5 (Poisson arrivals).** Their expected bounds use the memorylessness of geometric lengths and Harris's inequality, which Mathlib does not have.
- **Prefill.** The model gives a prompt no time, and so does the program. Chunked prefill would put the program outside the paper's hypotheses.
