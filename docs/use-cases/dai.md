# Throughput-optimal scheduling (Dai et al.)

Dai, Deng, Li, Peng, *Throughput-Optimal Scheduling Algorithms for LLM Inference and AI Agents* ([arXiv 2504.07347](https://arxiv.org/abs/2504.07347), v3, May 2026). This page writes the paper's serving system as a serQ program, states four of its propositions as claims of that program, and proves them in Lean about the program's paths, the stability theorem also for random arrivals. Issue #257.

## The paper

The paper models one LLM inference engine as a queue whose server processes batches of tokens. A request brings a prompt of $v_p$ tokens (prefill) and an output of $v_d$ tokens (decode, one token per batch). A batch holds at most $b_{\max}$ tokens and takes $t_b$ time units for $b$ tokens. The scheduler decides which tokens of which requests go into the next batch. The question is which schedulers keep the queue stable for every load the hardware can carry.

## Key contributions

- **The capacity of an engine** (Theorem 2(a)): no scheduler serves more than $b_{\max}/t_{b_{\max}}$ tokens per unit of time, so a load $\lambda(m_p + m_d)$ above it diverges.
- **Work conservation is enough** (Theorem 2(b)): every *work-conserving* scheduler (a batch is full whenever the requests could fill it, (8)–(9)) is stable below that load.
- **Which deployed schedulers are work-conserving** (§4): Orca and Sarathi-Serve are; FasterTransformer and vanilla vLLM are not, and they can diverge at loads the hardware carries.
- Extensions to multi-class, DAG and fork-join agent workloads (§5) and to a batch-size limit (§6), where even a work-conserving scheduler can be unstable.

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

`serve only` (#261) serves only the decodes while any request decodes, and only the prefills otherwise: decode first, no mixed batches.

## The key propositions

1. **Theorem 2(a), the rate.** Under any scheduler, at most $b_{\max}/t_{b_{\max}}$ tokens are served per unit of time. (The paper concludes divergence above that load by the strong law of large numbers on the arrivals; the pathwise content is the rate.)
2. **(8)–(9) for Sarathi.** Every batch is full whenever the requests could fill it: if $\sum_i (p_i + \mathbf 1\{p_i = 0\}) \ge b_{\max}$ then the batch has $b_{\max}$ tokens.
3. **§4 for FasterTransformer.** It is not work-conserving: some state has a batch below $b_{\max}$ although the requests could fill it.
4. **Theorem 2(b), pathwise.** The paper proves that a work-conserving scheduler makes the Markov chain positive recurrent below capacity, for random arrivals. Its deterministic core is Lindley's argument: with arrivals one gap apart and the load at most $b_{\max}/t_{b_{\max}}$, the work that has arrived and not been served stays bounded on every path, by $(b_{\max}+1)(v_p+v_d)$ tokens, however many requests arrive. It holds at the boundary itself, where the program runs.

## The propositions in serQ

Each is a `claim` of the program (`docs/language.md`, Claims):

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

`scripts/gen_lean_claims.py` reads the programs' IR (`tools/claims/*.ir.json`) and writes each claim as a Lean statement about the executable semantics (`lean/Serq/Claims.lean`). For Sarathi:

```lean
def work_conserving : Prop :=
  EveryIteration deployment family_work_conserving prog fun r => ((r.demand < 128) ∨ (r.stats.tokens = 128))
```

`EveryIteration D W P q` says: for every workload `w` in the family `W` and every machine `m` reached by a run of `P` on `w`, event by event (`Exec.Reach`), if an iteration is running then its record satisfies `q`. The family is any number of sessions (up to 500) with the program's arrival times. `examples/papers/ClaimsProved.lean`, also generated, checks that each claim has its proof in `examples/papers/Dai.lean`.

- **`token_rate`** holds for every program on this engine, so for every scheduler serQ can write. `Exec.served_rate` (`Serq/Claim.lean`) carries an invariant from event to event: the delays are sorted and in the future, the running iteration ends no earlier than now, and `served · T ≤ R · end` of the running iteration. It needs one fact about the deployment, that a batch of at most `budget` tokens lasts at least `T/R` per token. `staircase_rate` proves that for the staircase cost: $b \le b_{\max}$ and $b_0 \mid b_{\max}$ give $b\,(c + a\,b_{\max}/b_0) \le b_{\max}\,(c + a\lceil b/b_0\rceil)$.
- **`work_conserving`** is `Exec.work_conserving` (`Serq/Work.lean`). `every_iteration_of` reduces a claim over every iteration to the iteration's start. There `Fill.assign_eq_fillIter` says the batch is the greedy fill of the residents, and `fillAmounts_sum` that the fill takes `min budget demand`. The fill lemma needs that no job grows a hold. `Serq/Inv.lean` proves that every session runs a sub-program of its program on every path, and Sarathi's program has no `growing`.
- **`bounded`** (`examples/papers/DaiBounded.lean`) is an invariant carried over every event. A ghost state places each request in its program: not yet arrived, prefilling, decoding, or ended, with the tokens its job has left. The invariant ties that ghost to the sessions, jobs and delays, and keeps the balance `1280 · arrived + batch = served + backlog`. On top of it is Lindley's potential, scaled by the gap $G = 46\,750$ to stay in ℕ: $\Psi = G \cdot \text{backlog} + 1280 \cdot (s \bmod G)$ at an iteration start $s$.
    - A full batch lasts $\tau = 4675$ and serves 128 tokens, and $1280\,\tau = 128\,G$, so the phase it adds equals the work it removes, and $\Psi$ does not grow across it.
    - A partial batch serves all its residents' demand (`work_conserving`), so fewer than 128 residents are left, each with fewer than 990 tokens.
    - An idle engine has no backlog, and the next batch starts at one arrival.

  Each case keeps $\Psi \le G\,(b_{\max}+1)\cdot 1280$, and the claim follows at every iteration start. The family again allows up to 500 sessions, and $500 \cdot 1280 > 165\,120$, so the bound is not the trivial one.
- **Theorem 2(b) for random arrivals** (`examples/papers/DaiStable.lean`) is not a claim: claims are about the paths of one workload, and this is about a distribution over them. `Serq/Chain.lean` makes the program a Markov kernel. In each slot, one iteration of the engine, $k \le K$ requests arrive with probability $p_k$ (`Exec.inject` appends a session ready to run `dai_sarathi.sq`'s program after its arrival delay). The states are the machines reached from the empty one. The invariant of a slot's end, which Bari et al. share (`Serq/Slot.lean`), gives three facts:
    - a slot with a full batch changes the backlog by exactly $1280k - 128$ (`backlog_slot`);
    - a state whose batch is not full, or whose engine is idle, has backlog below $128 \cdot 1280$ (`backlog_lt_of_F`, from work conservation);
    - so below capacity, $1280 \sum_k k p_k < 128$, the backlog drifts down by $\varepsilon = 128 - 1280 \sum_k k p_k$ outside that set (`drift`).

  Foster's criterion (`Serq/Foster.lean`) then gives $\varepsilon \cdot E[\text{slots to reach } F] \le \text{backlog}$ from every state (`hitTime_le`), the expectation finite (`hit_tendsto`: its truncations converge to it), and a finite expected return time to $F$ (`returnTime_le`). Two examples check the theorem is not vacuous: one request makes a full batch, and one arrival per slot with probability 1/20 is below capacity.
- **Positive recurrence** (`examples/papers/DaiRecurrent.lean`). The machine's states keep the clock and every ended session, so the chain above never returns to a state. At a slot boundary the engine's future depends only on its job list, and `DaiChain.absSlot` is one slot on that list. `DaiSim.simulation` proves that the machine chain projects onto it exactly. On the chain on job lists the empty list is one state, and three facts hold below capacity:
    - its expected hitting time is bounded from every state (`hit_nil_le`): no arrival in a slot has probability above 0.9, and enough empty slots drain any state of `F`;
    - every state reaches every state (`irreducible`);
    - every state is positive recurrent (`positive_recurrent`).

  This is Theorem 2(b) for the chain whose state is the queue's content, which is the paper's chain, from `Serq/Recurrence.lean`: Foster's drift to `F`, plus a positive chance of reaching one state from `F`, gives positive recurrence. On the program's own machine chain (`DaiProgram`), whose states never repeat, the engine empties in bounded expected time from every state and again within one constant from every empty machine (`hit_idle_le`, `return_idle`).
- **`not_work_conserving`** is a witness. Two requests arrive 467.5 ms apart, and after 12 events (at 0.935 s) the first decodes alone while the second's 290 prompt tokens wait: `demand = 291`, `tokens = 1`. The machine is computed in the kernel (`decide +kernel`).

## On the run

`serq run`, seed 1, horizon 50 s (5 000 000 units of 10 µs):

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

The interpreter finds FasterTransformer's witness at the same instant as the Lean proof, 93 500 units. A request alone takes 990 × 4675 units (46.3 s), so 50 s shows the claims and not the contrast. Over 1000 s (`--horizon 100000000`) Sarathi ends 2 038 of 2 139 requests with about 98 live on average, and FasterTransformer ends 21 with about 1 059 live: it diverges where Sarathi does not. Over that run Sarathi's backlog peaks between 51 600 and 51 800 tokens (the claim with those bounds in place of 165 120 fails and holds), about a third of the proved bound. That is the steady state, not a warm-up overshoot: about 99 requests decode at once (Little: $990 \cdot 4675 / 46\,750$), and their remaining decodes, spaced ten tokens apart, hold about 49 500 tokens. The newest arrival and its prefill make up the rest. The peak does not grow with the horizon: the claim with 52 000 holds over 4 000 s. The load is $\lambda(m_p + m_d) = 1280/46750$, exactly $b_{\max}/t_{b_{\max}} = 128/4675$, the boundary of Theorem 2.

## What it leaves out

- **The machine chain's own states.** They are transient: the clock and the ended sessions never repeat. Positive recurrence is proved for the job-list chain and, on the machine chain, for the event that the engine is empty. That the machine's job list has the job-list chain's law is the one-step `simulation` plus Dynkin's criterion, not a Lean theorem.
- **Poisson arrivals: on the machine chain only.** `examples/papers/DaiPoisson.lean` proves Theorem 2(b) for a Poisson stream of rate $\lambda$: a slot brings the arrivals of the running iteration, a Poisson number of mean $\lambda$ times its cost, and below the paper's condition $1280\,\lambda\,t_{b_{\max}} < 128$ the empty engine is a positive recurrent atom of the machine chain (`return_idle`), as `DaiProgram.lean` proves for a fixed distribution. No chain on job lists is built for these arrivals, so positive recurrence of every job list is proved for a fixed distribution only. The program itself still writes `arrive gap(…)`, and the chain stamps an arrival with the running iteration's start rather than its own instant, so a latency read on it is up to one iteration longer; the job list is the program's. The divergence above capacity also needs the strong law on the arrivals.
- **Orca and vanilla vLLM.** Orca needs `serve by` and vanilla vLLM `exclusive prefill`, and the Lean fragment has neither. `Exec.work_conserving` would cover Orca once the fragment sorts residents by a key, since the fill conserves work in any order.
- **§5 and §6.** Multi-class and DAG workloads are written with `branch with` and `choose`, but the fragment has one engine. Fork-join needs a statement that creates sessions. The batch-size limit of §6 is a pool `cap` the program can write, and its stability region is not claimed.
