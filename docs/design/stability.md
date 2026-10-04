# Stability without a path measure

The stability theorems of the three papers (Dai et al. Theorem 2(b), Bari et
al. Theorem 2, Kong et al. Theorems 3.4–3.5) are statements about a Markov
chain: positive recurrence, or an expectation over Poisson arrivals. Mathlib
has neither Foster–Lyapunov nor fluid limits, and building a path measure for
`Exec` is a project of its own. This design reaches the same statements by
three steps, each of which is useful without the next (#305).
What the result does not yet say: the states keep the absolute clock and
every ended session (`inject` appends, nothing removes), so `F` is an
infinite set and the chain is not irreducible as it stands, and that finite
return times to a small set make an irreducible chain positive recurrent is
not proved.

## 1. Pathwise stability, as a claim

A stability theorem has a deterministic core: Lindley's argument that a
work-conserving server below (or at) capacity keeps its backlog bounded when
the arrivals stay inside an envelope. For arrivals one gap apart it needs no
probability at all, so it is a `claim` of the program:

```serq
claim bounded: every iteration of engine (arrived * (vp + vd) <= served + (bmax + 1) * (vp + vd));
```

`arrived` is a new iteration observable, the sessions the workload has
started by the iteration's start (`CtxVar::Arrived`; in Lean
`IterRec.arrived`, from `Workload.arriveSlot`). With `served` it gives the
backlog in tokens, which no earlier observable did: `residents` counts
requests, not work, and `now` counts arrivals that may never come.

The proof (`lean/Serq/Papers/DaiBounded.lean`) is an invariant over every
event, carrying Lindley's potential
$\Psi = G \cdot \text{backlog} + W \cdot (s \bmod G)$ at an iteration start
$s$, with $W$ a request's tokens and $G$ the gap. A full batch adds as much
phase as it removes work ($W\tau = b_{\max} G$). A partial batch, which is
partial only because the residents asked for less (`work_conserving`), leaves
fewer than $b_{\max}$ residents with less than $W$ each. An idle engine has
none. Each case keeps $\Psi \le G (b_{\max}+1) W$.

## 2. Foster's criterion on a kernel

A separate change (`lean/Serq/Foster.lean`) states a Markov chain by its kernel of finite support
and defines the expected hitting time of a set $F$ by its first-step
recursion, truncated: $h_0 = 0$ and $h_{n+1}(x) = 1 + \sum_y P(x,y)\,h_n(y)$
off $F$. The expected hitting time is the least nonnegative solution of that
equation and the limit of the truncations, so no path measure is needed. A
drift $\sum_y P(x,y) V(y) \le V(x) - \varepsilon$ off $F$ gives
$\varepsilon h_n \le V$ by induction (`drift_bound`), hence a finite expected
hitting time (`hitTime_le`) and a finite expected return time to $F$
(`positive_recurrent_of_drift`).

## 3. The bridge

`lean/Serq/Chain.lean` makes a program a kernel. A slot is one iteration of
the engine: `k ≤ K` sessions arrive with probability `p k`
(`Exec.inject` appends each, ready to run), and the engine moves to its next
iteration (`Exec.slot`). `Exec` itself is unchanged. The states are the
machines reached from the empty one, so a drift needs to hold only where the
program can be.

For Dai et al. (`lean/Serq/Papers/DaiStable.lean`) the Lyapunov function is
the backlog, and `F` is the set where the batch is not full or the engine
idle. Outside `F` a slot changes the backlog by exactly `1280 k − 128`, and
inside it the backlog is below `128 · 1280` (work conservation). Below
capacity Foster's criterion bounds the expected time to reach `F` by
`backlog / ε`. The phase term of step 1 is not needed: random arrivals are
counted per slot, not on a clock.

At most 10 000 sessions arrive in a slot. That is the proof's bound, not the
semantics': the proof follows one round of `Exec.drain`, which runs 10 000
sessions. The fuel of an instant itself runs out near 10⁷ arrivals
(`Exec.settle` repeats the round 1 000 times), and only beyond it would some
arrivals still be ready, not yet jobs, when the slot ends, and the drift be
false. The claims' families stop at 500 sessions for the same kind of
reason.

A slot is one iteration, not one unit of the paper's time, and an idle slot
lets no time pass. The capacity condition `1280 · E[k] < 128` is still the
paper's `λ (v_p + v_d) < b_max / t_{b_max}`: outside `F` every iteration is
full and lasts `t_{b_max} = c + a`, so the arrivals in a slot are those of
`t_{b_max}` units, `E[k] = λ t_{b_max}`.

Bari Theorem 2 is next, with the arriving requests' lengths drawn too. Kong
3.4–3.5 also need an expectation inequality (Harris).

## Self-critique

- **Bounding `residents`.** Rejected as the claim. Theorem 2(b) is about
  the number of requests, but a decode takes one token per batch, so about a
  hundred requests decode together at this load (101 at most on a
  1000 s run). A bound on requests needs the decode times as well as the work.
  The work bound is the one Lindley's argument gives directly.
- **`(vp + vd) * now <= (served + K) * gap`, without a new observable.**
  Rejected. It counts arrivals by the clock, so after the last arrival of a
  finite workload it keeps counting requests that never come. It held on
  every run only by slack (the last request ends about 101 gaps after it
  arrives, inside the 129 the bound allows), not by the argument, and the
  Lean families are finite.
- **Rate stability by the strong law.** A weaker intermediate result,
  `served / now → load` almost surely, from Mathlib's
  `ProbabilityTheory.strong_law_ae` and a pathwise Lindley inequality. Not
  taken, because step 1 plus step 3 gives the stronger statement and rate
  stability would not be reused.
