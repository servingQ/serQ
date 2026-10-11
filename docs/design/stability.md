# Stability without a path measure

The stability theorems of the three papers (Dai et al. Theorem 2(b), Bari et
al. Theorem 2, Kong et al. Theorems 3.4–3.5) are statements about a Markov
chain: positive recurrence, or an expectation over Poisson arrivals. Mathlib
has neither Foster–Lyapunov nor fluid limits, and building a path measure for
`Exec` is a project of its own. This design reaches the same statements by
three steps, each of which is useful without the next, and a fourth that
gives positive recurrence: of the chain on job lists the program's job list
follows, and of the empty engine on the program's own chain (#305).

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

The proof (`examples/papers/DaiBounded.lean`) is an invariant over every
event, carrying Lindley's potential
$\Psi = G \cdot \text{backlog} + W \cdot (s \bmod G)$ at an iteration start
$s$, with $W$ a request's tokens and $G$ the gap. A full batch adds as much
phase as it removes work ($W\tau = b_{\max} G$). A partial batch, which is
partial only because the residents asked for less (`work_conserving`), leaves
fewer than $b_{\max}$ residents with less than $W$ each. An idle engine has
none. Each case keeps $\Psi \le G (b_{\max}+1) W$.

## 2. Foster's criterion on a kernel

A separate change (`lean/Serq/Foster.lean`) states a Markov chain by its kernel:
from $x$, outcome $i$ has probability $p(x,i)$ and leads to $\mathrm{next}(x,i)$.
The outcomes may be infinitely many (a Poisson number of arrivals), so an
expectation is a series, and comparing two of them asks that they converge
(`Kernel.Integrable`; bounded functions, and every function when the outcomes
are finitely many, do). The kernel
defines the expected hitting time of a set $F$ by its first-step
recursion, truncated: $h_0 = 0$ and $h_{n+1}(x) = 1 + \sum_i p(x,i)\,h_n(\mathrm{next}(x,i))$
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

For Dai et al. (`examples/papers/DaiStable.lean`) the Lyapunov function is
the backlog, and `F` is the set where the batch is not full or the engine
idle. Outside `F` a slot changes the backlog by exactly `1280 k − 128`, and
inside it the backlog is below `128 · 1280` (work conservation). Below
capacity Foster's criterion bounds the expected time to reach `F` by
`backlog / ε`. The phase term of step 1 is not needed: random arrivals are
counted per slot, not on a clock.

Any number of sessions may arrive in a slot. `Exec.drain` runs one ready
session per round, and its fuel is the ready list's length and 10 000 more
for the sessions their commands make ready (`Exec.drainFuel`), so every
arrival runs its commands within the instant, as in the interpreter, whose
loop has no bound.

A slot is one iteration, not one unit of the paper's time, and an idle slot
lets no time pass. With arrivals in continuous time
(`examples/papers/DaiPoisson.lean`) the slot's law depends on the state: on
a busy engine the arrivals are those of a Poisson stream of rate `λ` while
the running iteration lasts, its `cost` (`Slot.Dur`), a Poisson number of
mean `λ · cost`; on an idle engine the next arrival starts an iteration.
Outside `F` every iteration is full and lasts `t_{b_max} = 4675`, so the
drift is `128 − 1280 λ t_{b_max}`, and the capacity condition is the
paper's `λ (v_p + v_d) < b_max / t_{b_max}` exactly. Inside `F` slots are
shorter, and Foster's criterion asks nothing of them. For Bari et al.
(`BariPoisson.lean`) each arrival also draws its type, a compound Poisson
list, whose work has mean `λ · 4640 · E[v_p + v_d]` (Wald's identity,
`Poisson.hasSum_compound_work`).

For Bari et al. (`examples/papers/BariStable.lean`) a slot's arrivals are a
list of drawn (prompt, output) lengths (`Exec.slotL`; `slot` is the list of
`k` copies). RAD is not work-conserving in Dai et al.'s sense, but its batch
is full unless every resident decodes and fewer than 128 do (`optimal_tiling`),
and that is all the drift needs: the same `F`, the same Lyapunov function, the
same proof. The invariant of a slot's end is one (`lean/Serq/Slot.lean`),
parameterised by where a request's lengths come from (constants, or
attributes 10 and 11), the prompt tile (1, or 128) and what the engine's
batch is; each paper proves only its batch (`ci_start`). Kong 3.4–3.5 also need an expectation inequality (Harris),
and are not done.

Bari et al.'s random planner over `g` nodes (`examples/papers/BariNodes.lean`)
is a product chain: a state is every node's machine, and a slot draws the
arrivals as above, routes each to a node uniformly, and runs one slot at
every node with the requests routed to it. A request reaches node `i` with
probability `1/g` (`mean_route`), so node `i`'s backlog has the drift
`128 − load/g` outside its own `F` (`drift`). The routing does not read the
state, so node `i` alone is `BariStable`'s chain with thinned arrivals
(`marginal`), and its chain on job lists is positive recurrent below
`load < 128 g` (`positive_recurrent`). The nodes' slots are synchronised: one slot is one iteration
of every node, which is this model's, not the paper's.

## 4. Positive recurrence

Step 3's states keep the absolute clock and every ended session (`inject`
appends, nothing removes), so the chain never returns to a state, and a
finite return time to the set `F` is not yet positive recurrence. Two
things close the gap.

**A chain on job lists, and the machine chain lumped onto it.** At a slot
boundary all the engine's future depends on is its job list: each job's
mode and left work in admission order (and, for RAD, the output a prefill
will decode). `DaiChain.absSlot` and `BariChain.absSlot` are one slot on
that list, written as plain list functions: new prefills join behind the
jobs, the batch is the greedy fill in admission order, a finished prefill
returns as a decode behind the rest. `DaiSim.simulation` and
`BariSim.simulation` prove that the projection of a machine to its job list
commutes with a slot, for every machine the chain reaches: the machine
chain is lumpable, and the empty list is one state.

**From a set to a state** (`lean/Serq/Recurrence.lean`). If the chain drifts
to `F`, and from every state of `F` reaches a target `T ⊆ F` within `L`
steps with probability at least `δ`, then the expected hitting time of `T`
is at most `V / ε + (L + B / ε) / δ`, `B` a bound on `P^L V` over `F`: each
visit to `F` is a trial that costs at most `L` steps and an expected return,
and succeeds with probability `δ` (`hit_le_of_reach`). The proof uses the
chain killed at `T`, `hit (m + L) ≤ L + Q^L (hit m)` and `Q^L 1 = 1 − reach`,
by induction on the truncation, so every bound holds uniformly in `n` and
no supremum in ℝ is read. Applied with `T` one state `o`, the expected
hitting time of `o` is bounded from everywhere; applied again with `F =
{o, y}`, every state `o` reaches is positive recurrent
(`positiveRecurrent_of_hit`).

`PositiveRecurrent` is the first-step recursion of the expected return time,
truncated and bounded in the truncation; that it is the path measure's
`E_x[τ_x⁺]` is the minimal-solution argument of `Serq/Foster.lean`'s header,
not a Lean theorem.

For both papers the target is the empty list. Below capacity a slot brings
no request with positive probability (for Dai `E[k] < 0.1`; for Bari every
request brings at least 129 tokens, so `E[#requests] < 128/129`), and a slot
without arrivals serves at least one token while there is a job. From a
state of `F`, whose backlog is bounded, enough empty slots drain the engine,
so `δ` is a power of the probability of an empty slot. The chain on job
lists is then irreducible (every state drains to the empty list, which
reaches every state by definition) and every state is positive recurrent:
Dai et al.'s Theorem 2(b) (`DaiRecurrent.positive_recurrent`) and Bari et
al.'s Theorem 2 (`BariRecurrent.positive_recurrent`), for the chain whose
state is the queue's content, as the papers' chains are.

**On the program's own chain** (`DaiProgram`, `BariProgram`). The machine
chain never revisits a state, since it keeps the clock and every ended
session, so its states are transient and positive recurrence is a property
of what it lumps onto. What recurs on it is an event: the engine is empty,
`σ m = []`. Every machine with an empty engine behaves alike from there (its
job list follows the job-list chain from `[]`), and on the machine kernel
itself the engine empties in bounded expected time from every state
(`hit_idle_le`), and empties again within an expected time bounded by one
constant from every machine whose engine is empty (`return_idle`): the
empty machines are a positive recurrent atom. That the machine's job list
has the job-list chain's law, step after step, is the one-step
`simulation` and Dynkin's criterion, not a Lean theorem; `DaiProgram` and
`BariProgram` do not need it, since they prove their bounds on the machine
kernel directly.

**For Poisson arrivals** (`DaiPoisson`, `BariPoisson`). The same two bounds
hold on the chains of §3 whose slots bring a Poisson number of requests.
No iteration outlasts a full batch (`dur_le`), so a busy slot brings no
request with probability `e^{-λ · dur} ≥ e^{-λ t_{b_max}}` (`p0_ge`), and
`δ` is again a power of one probability. A slot adds at most `1280 (λ
t_{b_max} + 1)` tokens in expectation (Bari: `(4640 λ + 1) E[v_p + v_d]`),
so `P^n V` converges for every `n` (`Kernel.integrableN_of_apply_le`).
`hit_le_of_reach` asks for that alone (`Kernel.IntegrableN V`): every other
function it takes an expectation of is bounded. Below the paper's capacity
the engine empties in bounded expected time (`hit_idle_le`) and the empty
machines are a positive recurrent atom (`return_idle`). No chain on job
lists is built for Poisson arrivals, so `positive_recurrent`, of every
state, is proved for a fixed distribution only.

**The `g` nodes at once** (`BariNodes`). The total backlog drifts down only
while every batch is full (`drift_sum`), and the set where some node's
batch is not full is unbounded, so it is no `F` for `hit_le_of_reach`. The
squares are: outside node `i`'s `F` its square drifts by `−2 ε V_i` and a
constant, inside it stays below a constant, so `Σ_i V_i²` drifts down by
one outside the set where every backlog is at most `R` (`driftQ`), a set of
bounded backlogs. From it, `R` slots without arrivals empty every node,
and the states with every node empty are a positive recurrent atom
(`return_idle`). This needs a slot without arrivals to have positive
probability, which `load < 128 g` does not give for `g > 1`: one request in
every slot is below two nodes' capacity, and after the first slot some node
always holds the newest request (it brings at least 129 tokens, a node
serves at most 128 a slot; an argument, not a Lean theorem). Poisson
arrivals give it. Without it the positive recurrence of the chain as a
whole is not proved: the state with every node empty is then not the atom
to use.

## Self-critique

- **A quotient of the machine states.** Rejected for step 4. Taking the
  clock out (`docs/design/stochastic-model.md`, Proposition 4(iii)) leaves
  the ended sessions, and a quotient that also forgets them and renames the
  live ones is a type of its own, with every `Exec` operation proved to
  respect it. The job list is that quotient's useful part, written as a
  plain list function, and a one-step simulation is all that ties it to
  `Exec`.
- **"Irreducible, and a finite return time to a finite set, give positive
  recurrence".** Not taken as the route. It needs the chain watched on the
  finite set and a renewal identity; `hit_le_of_reach` gives the same
  conclusion from the drift, a reach probability and the truncated
  recursions alone, and needs `F` neither finite nor irreducible.

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
