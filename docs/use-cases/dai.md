# Throughput-optimal scheduling (Dai et al.)

Dai, Deng, Li, Peng, *Throughput-Optimal Scheduling Algorithms for LLM Inference and AI Agents* ([arXiv 2504.07347](https://arxiv.org/abs/2504.07347), v3, May 2026). This page writes the paper's serving system as a serQ program, states four of its propositions as claims of that program, and proves them in Lean about the program's paths, the stability theorem also for random arrivals.

## The paper

The paper models one LLM inference engine as a queue whose server processes batches of tokens. A request brings a prompt of $v_p$ tokens (prefill) and an output of $v_d$ tokens (decode, one token per batch). A batch holds at most $b_{\max}$ tokens and takes $t_b$ time units for $b$ tokens. The scheduler decides which tokens of which requests go into the next batch. The question is which schedulers keep the queue stable for every load the hardware can carry.

## The serving system it assumes

The basic LLM queueing model of §3.1:

- discrete time; requests arrive and are indexed in arrival order;
- one engine; a batch is a set of prefill tokens and decode tokens with prefill before decode for each request (6a), at most one decode token per request (6c), at most $b_{\max}$ tokens (6d);
- a batch of $b$ tokens lasts $t_b = c + a\lceil b/b_0\rceil$ (Assumption 1), with $b_0$ dividing $b_{\max}$;
- no memory limit (the KV cache is not modelled).

The operating point of the program is the paper's point C (§6.2): CodeLlama-34B on one GPU, $t_b = 11.28 + 35.47\lceil b/128\rceil$ ms, $b_{\max} = 128$, one request every 467.5 ms with 290 prompt and 990 output tokens. The program has no batch-size limit: that is the §6 model, outside the propositions below.

## The system in serQ

```serq title="examples/papers/dai_sarathi.sq"
--8<-- "examples/papers/dai_sarathi.sq"
```

| Paper | serQ |
|---|---|
| batch budget $b_{\max}$ | `budget bmax;` |
| batch time $t_b = c + a\lceil b/b_0\rceil$ | `cost c + a * ceil(tokens / b0);` (time in 10 µs, so every number is an integer) |
| request $i$ with $v_p$, $v_d$ | `run engine prefill (vp); run engine decode (vd);` |
| deterministic arrivals | `arrive renewal(gap);` |
| Sarathi-Serve: decodes first, then the oldest prefills | `serve admission;`: residents are served in admission order, and without a chunk cap that order is decodes first (`Serve.serve_eq_decode_first`) |

FasterTransformer is the same program with one line changed:

```serq title="examples/papers/dai_fastertransformer.sq (the stage)"
stage engine : step {
  budget bmax;
  cost c + a * ceil(tokens / b0);
  serve only (decoders > 0 ? decoding : !decoding);
}
```

`serve only` serves only the decodes while any request decodes, and only the prefills otherwise: decode first, no mixed batches.

## The key propositions

Let $T=t_{b_{\max}}$ and let $S(t)$ count tokens served before an iteration
starting at time $t$. The following results use the batch-time model above,
with $b_0$ dividing $b_{\max}$.

!!! theorem "Theorem 2(a) — Token-rate bound"
    For any scheduler obeying the batch budget and cost model,

    $$S(t)\,T \le b_{\max}\,t.$$

    This is the pathwise rate bound. Concluding divergence above capacity
    also requires a result about the arrival process.

!!! proposition "Work conservation — Sarathi and FasterTransformer"
    Sarathi fills the batch whenever resident demand can fill it:

    $$\mathrm{demand} \ge b_{\max}
      \quad\Longrightarrow\quad \mathrm{tokens}=b_{\max}.$$

    The FasterTransformer program has a counterexample: a batch of one
    decode token while another request's 290 prefill tokens wait.

!!! theorem "Deterministic backlog bound — The program's operating point"
    In `dai_sarathi.sq`, each request brings $v_p+v_d=1280$ tokens, arrivals
    are $46\,750$ time units apart and a full batch serves 128 tokens in
    4 675 units. At every iteration start,

    $$\mathrm{arrived}\,(v_p+v_d)-S(t)
      \le (b_{\max}+1)(v_p+v_d)=165\,120.$$

    The Lean claim covers up to 500 sessions with these parameters. This
    deterministic bound at capacity is distinct from stochastic positive
    recurrence strictly below capacity.

## The propositions in serQ

The two programs express these results as [claims](../api/program.md#claim):

```serq
// (8)-(9): a batch is full whenever the residents could fill it.
claim work_conserving: every iteration of engine (demand < bmax || tokens == bmax);

// Theorem 2(a): no scheduler serves more than b_max / t_{b_max} tokens per unit of time.
claim token_rate: every iteration of engine (served * (c + a * bmax / b0) <= bmax * now);

// §4 (dai_fastertransformer.sq): FasterTransformer is not work-conserving.
claim not_work_conserving: some iteration of engine (demand >= bmax && tokens < bmax);

// Theorem 2(b), pathwise: arrived and unserved work stays below b_max + 1 requests' worth.
claim bounded: every iteration of engine (arrived * (vp + vd) <= served + (bmax + 1) * (vp + vd));
```

`demand` is the paper's $\sum_i (p_i + \mathbf 1\{p_i=0\})$: what the residents could take with an unlimited budget. `served` is the tokens of the earlier iterations and `now` the start of this one, so `token_rate` says the served tokens never exceed the rate $b_{\max}/t_{b_{\max}}$ times the elapsed time. `arrived` is the requests that have arrived by the iteration's start, so `arrived * (vp + vd) - served` is the backlog in tokens. `some iteration` claims existence: one workload of the program's family and one iteration of one of its paths.

## The proof in Lean

The claim generator reads each program's IR and produces statements in
`lean/Serq/Claims.lean`. Universal claims cover every reachable path in the
generated workload family, with up to 500 sessions. The FasterTransformer
claim instead supplies an existential witness.

| Result | Proof | Argument |
|---|---|---|
| Token rate | `Dai.lean`, `token_rate` | Each batch's cost bounds its token rate; summing over iterations gives the elapsed-time bound. |
| Sarathi work conservation | `Dai.lean`, `work_conserving` | The greedy fill serves `min(budget, demand)`. This program has no growing holds. |
| Deterministic backlog | `DaiBounded.lean`, `bounded` | A backlog potential cannot increase across a full batch; a partial batch has fewer than 128 residents left. |
| FasterTransformer counterexample | `Dai.lean`, `not_work_conserving` | Kernel evaluation of two requests finds `demand = 291`, `tokens = 1` at 0.935 s. |

All proof files are under `examples/papers/`. See [the Lean guide](../lean.md)
for generation and build commands.

### Random arrivals

These results use separate Markov kernels, not the deterministic workload
written in the `.sq` file.

!!! theorem "Theorem 2(b) — Fixed slot-arrival distribution"
    Suppose each slot brings a bounded random number $k$ of requests with
    the program's fixed token lengths. If

    $$1280\,\mathbb{E}[k]<128,$$

    every reachable state of the job-list chain is positive recurrent.

    Proof: `DaiRecurrent.lean`, `positive_recurrent`, using the backlog
    drift proved in `DaiStable.lean`.

A full batch changes backlog by $1280k-128$. Below capacity this gives
negative drift outside the set of idle or partially filled batches.
The probability of a slot with no arrivals is positive, allowing the
chain to return to the empty job list.

For Poisson arrivals of rate $\lambda$ per time unit, `DaiPoisson.lean`
proves bounded expected return time to an empty engine when
$1280\lambda\cdot4675<128$. This result is on the machine chain;
positive recurrence of every job-list state is proved only for the fixed
slot-arrival distribution.

## On the run

Run both programs with their default seed and 50 s horizon (5 000 000
units of 10 µs):

```bash
serq run examples/papers/dai_sarathi.sq --horizon 5000000 --warmup 0 --seed 1
serq run examples/papers/dai_fastertransformer.sq --horizon 5000000 --warmup 0 --seed 1
```

Claim-table excerpts, Sarathi first:

```
claim            kind             result
work_conserving  every iteration  holds (1060 iterations)
token_rate       every iteration  holds (1060 iterations)
bounded          every iteration  holds (1060 iterations)
```

```
claim                kind             result
not_work_conserving  some iteration   witnessed at 93500.0000 (1060 iterations)
token_rate           every iteration  holds (1060 iterations)
```

The interpreter finds the counterexample at 93 500 time units, the same
instant as the Lean witness. A finite run checks the claims on its observed
path; it does not prove stability or divergence.

## What it leaves out

- **State recurrence.** A machine retains its clock and ended sessions, so
  complete machine states do not repeat. Recurrence results refer to the
  job-list chain or to returns to an empty engine. The one-step projection
  is proved in `DaiSim.lean`; equality of the projected process's law is
  not a Lean theorem.
- **Arrival timing.** The Poisson construction groups arrivals over an
  iteration and timestamps them at its start. Latencies can therefore be
  up to one iteration longer than with individual arrival times.
- **Other schedulers and workloads.** The proofs here do not establish
  stability for Orca, vanilla vLLM, multi-class, DAG or fork-join workloads,
  or the paper's model with a batch-size limit. The executable fragment
  has one engine.
