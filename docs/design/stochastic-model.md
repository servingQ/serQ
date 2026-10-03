# The deployment as a stochastic process

[Philosophy](philosophy.md) names the mathematics in one line, "a stochastic
process over resources whose definition is the object of theorems", and the
paper (`serving-queue-theory`, §2) works with a prefill queue, a decode
batch and a memory pool. Neither is the process. The line is a name, and
the paper's model is a *projection* of the process onto three numbers, with
the hit/miss mixture as an input. This document defines the process: a
serQ program as a generalised semi-Markov process, with the state read off
the interpreter (§2); what the definition proves and what it leaves open
(§3); the paper's model as a projection, with the three places the
projection is not Markov named (§4); the structure of a production
deployment (agents, router, engines, links) as a composition of such
processes (§5); and what the language does not yet say that the definition
needs (§6).

Status: 2026-10-03, a definition. It adds no IR node; it adds one Lean
module, `lean/Serq/Regen.lean`, for Proposition 5's eviction step.

## 1. The shape: a deterministic machine in a random environment

A production deployment has a boundary. Inside it nothing is random: the
scheduler is a function of its state (`schedule()` reads `running`,
`waiting` and the block pool, and the same state gives the same output),
the KV manager is a function of its state, the router is a function of what
it has observed (the correspondence, line by line, is `docs/language.md`
§7). Randomness crosses the boundary at two places:

- **The clients.** When a session arrives, what it sends (prompt and output
  lengths), how long it thinks between turns, whether it comes back.
- **The device.** How long an iteration takes. In every shipped program
  this is a fitted function of the batch (`cost`), so it is deterministic
  too; the 2.7–5.7 % MAPE of the A100 fit (`docs/language.md` §8) is the
  part of the device the model does not carry.

The language does not enforce the boundary: a `cost`, `budget`, `chunk`, a
`ps` capacity, a hold's units or a `cache` clause may contain a `~`
(`Program::validate` refuses a draw only in a queue key and a `serve` key,
`src/ir.rs:795`, `:829`), and then the machine itself is random. No
program in `examples/` does this, and §6 asks whether the linker should
refuse it.

So the process has the form

$$X(t) \;=\; \Phi\bigl(X(t_0),\ \xi\,[t_0,t]\bigr), \qquad t_0 < t,$$

where $X$ is the state of the deployment, $\xi$ is the environment (the
arrival instants with their marks, the end of each think time) and $\Phi$
is a deterministic map. Given a realisation of $\xi$ the path is a fact,
not a sample. That is the content of the oracle result
(`docs/validation.md` §3): drive the real scheduler and the program with
the same $\xi$, the 333-session trace, and the 3 321 first-token times
agree because both are the same $\Phi$. In the paper's model the hit or
miss of a turn is a Bernoulli variable with the policy's hit rate $h$; in
the process it is a function of the path, $\mathbb 1\{\text{the entry survived}\}$,
and §4 is about the distance between the two.

## 2. Definition

**At a glance.**

| | | |
|---|---|---|
| Deployment (Def. 1) | $(\mathcal M, \mathcal S, R, \mathcal W)$: pools with capacity, block, eviction order $\prec_m$ and preemption rule; stages (`fifo`, `ps`, `delay`, `step`); the session program; the workload (arrival law, turn kernel $G$) | the IR as data |
| Configuration (Def. 2) | $X = (t, \mathcal E, (\sigma_i), (P_m), (Q_s))$: clock, pending events, sessions (attributes, program counter, holds), pools (allocation, the *ordered* cache $\mathcal C_m$, queue), stages (jobs, residents, iteration, estimator $\hat\theta_s$) | `Interp`, field for field |
| Environment (Def. 3) | $\xi$: five independent uniform streams; every `~` reads one | the only randomness |
| Kernel (Def. 4) | $X' = \mathsf{start} \circ \mathsf{settle} \circ \mathsf{apply}_{e^\ast}(X)$ at the earliest event, ties by sequence number | deterministic |
| Process (Def. 5) | $X(t) = \Phi_t(\xi)$, a generalised semi-Markov process, total: a path either has finitely many transitions in $[0,t]$ or ends in the error state the run time reports | the object the theorems are about |
| Prop. 1 | $\Phi$ deterministic; one seed fixes, across machines, the arrivals and the marks of every (session, turn) | proved |
| Prop. 2 | $U_m + \lvert\mathcal C_m\rvert \le M_m$ on every path | proved for `Core.Step`'s five commands; asserted on every run of the interpreter |
| Prop. 3 | the prefill queue is a FIFO queue whose per-slot capacity is $B - d_\ell$ | proved per iteration by `Fill.lean`; the order step argued |
| Prop. 4 | Markov on the configuration space; a countable chain at integer times under `cost 1` and integer marks | proved |
| Prop. 5 | dead cache entries do not change what a live session sees: the process regenerates when empty | eviction step proved (`Regen.lean`); path lift a conjecture |

The notation follows the lecture (`serving-queue-theory/lectures/…/lecture1.tex`:
L1:def:workload's turn kernel $G$, L1:def:costs' step time $\tau$,
L1:lem:paths' request times) and the Lean model (`lean/Serq/Core.lean`,
`Exec.lean`) where they have a name for the thing. Sessions are $\sigma$,
stages $s$, pools $m$, engines $r$, iterations $\ell$, jobs $q$, requests
(residents) $k$; `B` is the program's token budget, as in the programs,
not the lecture's link bandwidth.

**Definition 1 (Deployment).** A deployment is $(\mathcal M, \mathcal S, R, \mathcal W)$:

- pools $m \in \mathcal M$, each with a capacity $M_m \in (0, \infty]$, a
  block $b_m$, an eviction order $\prec_m$ (a total order on cache entries:
  `lru`, or the keys of `evict by` then release order), a preemption rule
  $\mathrm{pre}_m \in \{\text{none}, \text{lifo}\}$, a waiting-selection
  order (`queue by`, keys then enqueue order) and optionally the step stage
  that admits for it (`admit via`);
- stages $s \in \mathcal S$, each of one kind: $\mathrm{fifo}(c)$,
  $\mathrm{ps}(\varphi)$ (a `ps` of constant capacity that some run holds
  with another is *shared*, and its jobs are flows under the program's
  `share`), $\mathrm{delay}$, or
  $\mathrm{step}(B, \mathrm{chunk}, \mathrm{serve}, \tau, m_s)$ with a
  token budget $B$, a chunk cap, a serving rule (an order `by (…)`, or
  `exclusive prefill`), an iteration cost $\tau : \text{batch} \to \mathbb R$
  and a memory pool $m_s$;
- a session program $R$, a tree of blocks of `CStmt` (`src/ir.rs:309-360`:
  `Turn`, `Set`, `Observe`, `Hold`, `Grow`, `Drop`, `Release`, `Load`,
  `Run`, `Branch`, `Loop`, `Choose`, `End`). `Route` (`Core.lean`) is the
  fragment of it the Lean model formalises, and `Route.wf` (every loop
  body contains a `run`) is the Lean model's well-formedness condition;
- a workload $\mathcal W$: an arrival law (`poisson`, `renewal`, `closed`,
  `batch`, explicit sessions) and a turn kernel $G$ (the `init` and `turn`
  blocks: a session's next attributes given its current ones, with draws).

The IR (`docs/ir.md`) is this tuple as data. `Program::validate` checks
its indices, moments, flows, and that every path through a loop body
reaches a `run`, a `hold` whose body does, or `end` (`Validator::lets_time_pass`,
`src/ir.rs`): the syntactic half of Lemma 1's hypothesis; the run time
holds the other half (§6, Outcome).

**Definition 2 (Configuration).** A configuration is

$$X \;=\; \bigl(t,\ \mathcal E,\ (\sigma_i)_{i \in I},\ (P_m)_{m \in \mathcal M},\ (Q_s)_{s \in \mathcal S}\bigr),$$

with $t \in \mathbb R_{\ge 0}$ the clock, $\mathcal E$ a finite set of
pending events each tagged $(\text{time}, \text{seq}, \text{kind})$ with
an absolute time (the kinds: `Arrive`, `Finish`, `IterEnd`, `LeaseEnd`,
and `EndWarmup`, which only starts the statistics; `Ev`,
`interp.rs:25-44`), $I$ the live sessions, and:

| Component | Contents | `interp.rs` | vLLM (`docs/language.md` §7 for the lines) |
|---|---|---|---|
| $\sigma_i$ | attributes $y_i \in \mathbb R^{A}$; continuation: a stack of (block, pc, frame kind); status $\in \{\text{ready}, \text{queued}(m), \text{at}(s), \text{growing}(m), \text{ended}\}$; the pending hold while queued (its pools, the unit and `reserve` expressions, the time it queued); holds, each with allocated units $u_{i,m}$ and computed position $p_{i,m}$ per pool; admission number $\nu_i$; leases; position at last preemption and the `stuck` flag; the trace or script cursor; the last token's time (for ITL) | `Session`: `attrs`, `frames`, `status`, `pending`, `holds.{pools,pos}`, `adm_seq`, `leases`, `preempt_pos`, `stuck`, `trace`/`script`, `last_token` | `Request`: `status`, `num_computed_tokens`, `output_token_ids`; its blocks in the KV manager's per-request table; its index in `running` |
| $P_m$ | used units $U_m = \sum_i u_{i,m}$; holders in admission order; the cache $\mathcal C_m$: entries $(r, u, t^{\mathrm{rel}}, n^{\mathrm{rel}}, \hat y)$ — owner (a session serial, live or ended, or a dead remainder), units, release time, release number, and the snapshot of the owner's attributes that `evict by` keys read; the waiting holds in queue order, each marked resumed or not; the stalled growers | `PoolState`: `used`, `holders`, `entries` (`CacheEntry.snap`), `queue`, `growers` | the block pool's free queue (the LRU order) and hash table; `Scheduler.waiting` |
| $Q_s$ | fifo: servers' jobs and queue; ps: virtual clock and finish tags; delay: jobs; step: residents in admission order, the running iteration's assignment (job $\mapsto$ tokens) and epoch; shared: the flows and their rates; and the stage's online estimator $\hat\theta_s$ (last start, EWMA gap, service and wait, count), which `est_lambda`, `est_rho`, `est_wait` and `price` read | `StageState`, `Kind`, `Job`, `Iter`, `Flow`, `est: PriceEstimator` (`stats.rs:333`; read at `interp.rs:3222-3254`) | `Scheduler.running`, `SchedulerOutput.num_scheduled_tokens`; a router's load picture |

`Interp` (`src/engine/interp.rs:408`) is this tuple with the report's
statistics appended (`evicted_entries`, `spills`, the time averages, which
no expression reads) and the five generators of Definition 3. The
estimator $\hat\theta_s$ is in $X$ and not among the statistics because a
`choose` or `evict by` key may read it: it is §1's "the router is a
function of what it has observed", as state. The vLLM column names
the object the component corresponds to; the line-by-line claims, with
their `file:line` citations, are in `docs/language.md` §7 and are not
repeated here. The cache $\mathcal C_m$ is the component the queueing model
does not have: an *ordered* set (by $\prec_m$) of entries tagged with the
one session that may read each, and "will $i$'s next turn hit" is a
question about $i$'s entry's rank in that order against the allocations to
come.

**Definition 3 (Environment).** Let
$\xi = \bigl(\xi_{\mathrm{arr}},\ \xi_{\mathrm{trace}},\ \xi_{\mathrm{evict}},\ \xi_{\mathrm{mach}},\ (\xi_{\mathrm{wl}}(i,k))_{i,k},\ (\xi_{\mathrm{sess}}(i))_i\bigr)$,
independent i.i.d. sequences of $U(0,1)$ variables, one per name, under
the product measure. Every draw `~d(…)` in the program consumes the next
element, or the next few (`~h2`, `~erlang`), of one stream (the
interpreter's four generators, `interp.rs:564-568`; a session's two,
`substream`, seeded from (seed, serial, turn, kind); which draw reads
which, `Interp::rng` and the `Sample` arm of `eval`; the inverse
transforms, `dist.rs:61`): the arrival law from $\xi_{\mathrm{arr}}$, the
choice of a trace session for an unordered `trace` from
$\xi_{\mathrm{trace}}$ (`interp.rs:899`), eviction keys and spill
predicates from $\xi_{\mathrm{evict}}$, a draw in a `cost`, `budget`,
`chunk` or `ps` capacity from $\xi_{\mathrm{mach}}$, the `init` and `turn`
blocks of session $i$ at its turn $k$ from $\xi_{\mathrm{wl}}(i, k)$, and
the statements of session $i$ from $\xi_{\mathrm{sess}}(i)$. Nothing else
in the semantics reads $\xi$. An ordered trace with `arrive batch` and no
`~` reads none of it: $\xi$ is the file.

**Definition 4 (Kernel).** Let $e^\ast = \min \mathcal E$ in the
lexicographic order (time, seq). The transition at $e^\ast$ is

$$X' \;=\; \mathsf{start} \circ \mathsf{settle} \circ \mathsf{apply}_{e^\ast}\,(X),$$

- $\mathsf{apply}_e$: `Arrive` creates $\sigma_i$ with $y_i \sim G(0, \cdot)$
  and status ready, and under `poisson`/`renewal` schedules the next
  arrival (`closed(n)` creates a session when one ends, `batch` and
  explicit sessions create all of them at $t = 0$; `interp.rs:640-658`);
  `Finish(s, q)` removes job $q$ from $Q_s$ and makes its session ready;
  `IterEnd(s)` commits the iteration's tokens (every resident's remaining
  work and position $p$ advance by its assignment) and marks $Q_s$ idle;
  `LeaseEnd` returns a leased allocation to $\mathcal C_m$.
- $\mathsf{settle}$: iterate one round to a fixed point, where a round
  (i) runs every ready session, in the order they became ready, until it
  blocks — a command (`set`, `observe`, `branch`, `choose`, `turn`, the
  entry of `hold` and of `run`) takes no time; (ii) for every pool not
  marked `admit via`, retries its growers in stall order and then its
  queue's selected head: admit iff $U_m + \max(u, \mathrm{reserve}) \le M_m$
  for every pool of the hold, evicting $\mathcal C_m$ in $\prec_m$ order
  until $U_m + |\mathcal C_m| \le M_m$ (`Core.lean`'s `[Admit]`,
  `Exec.lean`'s `makeRoom`), the first head that does not fit blocking
  the rest. An admitted session becomes ready and its body runs in the
  *next* round, after every pool's admissions of this one
  (`interp.rs:800-819`); the order matters, since the body's releases
  set the LRU order (#249).
- $\mathsf{start}$: for every step stage $s$ that is idle and has
  residents or a non-empty queue it serves, and only if no event in
  $\mathcal E$ has time $t$: order the residents by `serve`; hand out $B$
  greedily (one token to a decoder, up to `chunk` to a prefiller), growing
  each `growing` hold to the position it will reach, preempting under
  $\mathrm{pre}_{m_s}$ if a growth fails; then admit from the queues it
  serves while budget is left and the hold fits, each admitted session
  running its body until it joins this stage or blocks and taking its
  first tokens from the budget left (`admit_bound`, `interp.rs:2864`);
  schedule `IterEnd` at $t + \tau(\text{batch})$.

A `fifo`/`delay` job's `Finish` is scheduled at its start, a `ps` job's at
every change of the stage's population (virtual time: Glynn's
state-dependent clock speed, written as rescheduling), a lease's end at
the lease. Where any of these maps draws, it reads $\xi$ per Definition 3.

**Lemma 1 (every instant settles).** Call a statement *blocking* if it is
a `run` of positive work (the session waits for its `Finish`, or on a step
stage for the `IterEnd`), a `hold` that does not fit, a `grow` that
stalls, or `end`. For every program the linker accepts, $\mathsf{settle}$
either reaches its fixed point in finitely many rounds or ends the run in
the error state $\bot$, naming a session; it reaches the fixed point
whenever every pass of every session through a loop body reaches a
blocking statement.

*Proof.* A ready session executes commands until a blocking statement.
Between two blocking statements it executes finitely many when every pass
through a loop body reaches one, and outside loops the program is finite.
An admission round admits at least one hold or changes nothing; there are
finitely many waiting holds, and a session re-enters a queue only through
a statement, which again precedes a blocking statement. When the
hypothesis fails, a session executes without bound at one instant or is
re-readied without bound, and the run time's two counters
(`STEPS_PER_INSTANT`, `READIES_PER_SESSION`, `interp.rs`) end the run in
$\bot$. $\square$

The hypothesis is about paths. The linker enforces its syntactic half:
every path through a loop body must reach a `run` (a constant zero work
does not count), a `hold` whose body does, or `end`
(`Validator::lets_time_pass`), which is stronger than the Lean `wf`
(`Core.lean:113-122`, `hasRun`): `hasRun` is an OR over the arms of a
`branch` (`Core.lean:107`), so a run on an arm never taken counts there and
not here. What the linker cannot see is a `run` whose computed work is 0
on every pass (`start_job` completes it at once, `interp.rs`), or a `hold`
that fits, grows past its pool and preempts itself back into its own queue
at the same instant; those the counters catch. Three programs that passed
`wf` and ran forever before the two rules (verified, 2026-10-03), now a
link error, a link error and a run-time error respectively:

```
session { loop { set w = w + 1; } }
session { loop { branch (w > 0) { run tool (w); } else { set w = w; } } }
session { loop { run tool (w); } }           // with w = 0 from init
```

Making a zero-work run a transition (a `Finish` at $t$) would not have
helped: the failure would have moved to Definition 5 as infinitely many
transitions at one instant. Termination of $\mathsf{settle}$ is progress
at one instant, not in time: a hold whose body can never fit preempts
itself at every iteration and re-executes forever, each iteration lasting
$\tau$ (`docs/language.md` §3, the `stuck` counter).

**Definition 5 (The process).** $X(t)$ is the configuration after the last
transition at time $\le t$, with the convention that a transition at $t$
is complete (a scheduler step sees every arrival up to it). Suppose
Lemma 1's hypothesis, $\inf \tau \ge \tau_0 > 0$ over all batches,
finitely many arrivals in $[0, t]$ (almost surely for `poisson` and
`renewal`; by construction otherwise), and for the runs on `fifo`, `ps`,
`delay` and shared stages either a deterministic lower bound $w_0 > 0$ on
every positive work or, for drawn works, independent draws stochastically
bounded below by one law without an atom at 0. Then almost surely finitely
many transitions occur in $[0, t]$, and $X(\cdot)$ is a right-continuous
piecewise-constant path. The count: the sessions alive in $[0, t]$ are
finitely many; a step stage's runs are paced by `IterEnd`, at most
$\lceil t / \tau_0 \rceil$ per stage whatever their work; a run on another
stage lasts at least its work over the stage's greatest per-job rate
$r_{\max}$ ($1$ for `fifo` and `delay`; $\sup_n \varphi(n)/n$ for `ps`,
which is $\varphi$ itself for a constant $\varphi$ and exceeds 1 when
$\varphi$ exceeds the population; the capacity for a shared stage), which
the hypothesis takes finite — a lower bound on *time* needs the rate
bounded above, not the work alone — so a session's `Finish` clocks there
are at most $\lceil t\, r_{\max} / w_0 \rceil$ or, for drawn works, finitely many
a.s. because the partial sums of such a sequence diverge; zero-work runs
are finitely many per pass by Lemma 1;
`LeaseEnd` clocks are at most the holds executed, each preceding a
blocking statement; `EndWarmup` is one. Each clock transition is followed
by one $\mathsf{settle}$, finite by Lemma 1. Write

$$X(t) \;=\; \Phi_t(\xi).$$

The hypotheses are met by the shipped programs: `vllm.sq`'s `tool
(~exp(Z))` is i.i.d. exponential (no atom at 0), its engine runs are
paced by $\tau \ge$ `omega`, and `vllm_request.sq`'s `decode (o - 1 - …)`
of zero work at $o = 1$ completes at once, finitely often per turn.
Without the bounds the path can be Zeno: `loop { set w = w / 2; run tool
(w); }` makes infinitely many transitions before twice its initial `w`.
The run time holds two of the bounds and not the third: an iteration that
schedules tokens at a `cost` of 0 is an error (`start_iteration`,
`interp.rs`), so $\tau > 0$ on every iteration that does work, and the
step that only preempted may cost 0 but is bounded by Lemma 1's counters;
a positive *lower* bound $\tau_0$ and the work bound $w_0$ are
hypotheses, since the halving loop has positive clocks at every step.
For a `cost` affine in its variables with non-negative coefficients, which
every shipped one is, $\tau_0$ is its value on the empty batch; the Lean
fragment demands that constant to be at least 1 (`docs/lean.md`).

$(X(t))_{t \ge 0}$ is a **generalised semi-Markov process** in the sense of
Glynn (1989, *Proc. IEEE* 77(1):14–23): the discrete part of $X$ is the
state, the times in $\mathcal E$ are the clock readings, and Definition 4
is the transition at the earliest clock. Three departures from that
formalism, all routine: the state space is not countable (Proposition 4),
simultaneous clocks are resolved by the sequence number rather than
excluded, and every clock's setting distribution is degenerate except for
those we call *environment clocks* (the next arrival, a `delay` run of
drawn work: `tool (~exp(Z))`), which read $\xi$. Those set by the state
(`IterEnd` at $\tau(\text{batch})$, `Finish` at the work, a ps finish tag,
a lease) are functions of $X$. A `~` in a program is an environment clock
or a mark, and nothing else is random.

## 3. What follows from the definition

**Proposition 1 (Determinism; common random numbers).** $\Phi_t$ is a
measurable function of $\xi$, and every observation and gauge of a run is
$f(\Phi_\cdot(\xi))$. Let two deployments differ only in the machine (a
key, a budget, a cap: anything outside $\mathcal W$) and run on one seed.
Then (i) under `poisson`, `renewal`, `batch` or explicit sessions, if the
arrival expression reads no pool or stage observable, the arrival instants
coincide for every session, and so do the attributes of explicit sessions
and of an ordered trace, and the trace session an unordered trace picks
at each arrival ($\xi_{\mathrm{trace}}$ alone, in arrival order,
`interp.rs:899`); (ii) the `init` and `turn` marks of every (session,
turn) coincide, whatever either schedule did before it; (iii) a session's
own draws (`tool (~exp(Z))`, a `branch with`) coincide in program order
until the session's own path diverges; (iv) under `closed(n)` not even (i)
holds, since a session is created when one ends (`interp.rs:1039`).

*Proof.* Definition 4 is a composition of total functions of $(X, \xi)$,
Definition 3 fixes which stream each draw reads, and the streams are
independent. (i): the arrival law reads $\xi_{\mathrm{arr}}$ alone, in
arrival order, which the machine does not set. (ii): the `init` and
`turn` blocks of session $i$ at turn $k$ read $\xi_{\mathrm{wl}}(i, k)$,
a stream of their own seeded from (seed, $i$, $k$) at that `turn`
(`substream`, `do_turn`, `interp.rs`), which no other draw reads. (iii):
the session's statements read $\xi_{\mathrm{sess}}(i)$ in the order the
session reaches them, which is the same on both paths until its own
`branch` or `choose` reads the machine. $\square$

Before the per-session streams (2026-10-03) `init` and `turn` of every
session shared one generator, and a machine that changed how many `turn`
draws preceded an arrival shifted that arrival's draws: on `vllm.sq`, seed
1, `B` = 8192 against 512, 424 of 570 sessions drew a different `init`
value and 967 of 4 733 common (session, turn) pairs a different `n`
(`tests/settle.rs` holds the new property: 0 of them). What one seed fixes
across machines is now the *workload*, which is what a policy comparison
wants: `routing.sq`'s five policies under one seed are a sweep over one
workload, and the oracle's `first_divergence.sh` is the search for the
instant in (iii) between the program and the real scheduler, whose
workload is a trace and draws nothing. The *machine* may read $\xi$ too:
an eviction key or spill predicate that samples reads $\xi_{\mathrm{evict}}$,
and a `cost`, `budget`, `chunk`, `ps` capacity, hold unit or `cache`
clause that samples reads $\xi_{\mathrm{sess}}$ (`interp.rs:2799`,
`Which::Session`), interleaved with the sessions' own draws. No shipped
program does either; one that does has a scheduler that is not a function
of its state, and Proposition 1 (iii) fails for it at the first such draw.

**Proposition 2 (Memory invariant).** For every $t$ and every
$m$: $U_m(t) + |\mathcal C_m(t)| \le M_m$.

*Status.* Proved for the five commands of the relational pool semantics,
queue, admit, free, drop and leave (the relation `Step`, `Core.lean:286`;
the theorem `SerqLang.Step.invariant`, `Core.lean:426`); for the
executable fragment, the eviction loop makes
the room it is asked for or empties the cache and never touches the
allocation (`Exec.makeRoom_room`, `makeRoom_used`), and the commands the
fragment adds — `grow`, preemption, `reserve` — have no invariant theorem
yet. `lease`, `load` and `spill` are outside the fragment. For the
interpreter the statement is checked, not proved: a debug assertion at the
end of every $\mathsf{settle}$ holds it for every pool on every test,
oracle and differential-test run (§6, item 6).

**Proposition 3 (The prefill queue is a FIFO queue in the environment of
the decode batch).** Consider one step stage with `serve admission`, no
chunk cap, $d_\ell \le B$ decoding residents at the start of iteration $\ell$,
nobody waiting at the pools it admits for, distinct owners for its
residents, every `growing` resident's allocation already covering the
position it reaches in the iteration (no block boundary crossed), and no
preemption of the prefills considered — all of it at every iteration
$\ell_0 < \ell' \le \ell$ below. Let $\mathrm{ahead}_\ell(k)$ be the tokens iteration $\ell$
gives to the prefilling residents admitted before $k$,
$\min(\sum_{k' \prec k} \mathrm{rem}_{\ell}(k'),\ B - d_\ell)$. Then resident $k$
receives $\min(\mathrm{rem}_\ell(k),\ B - d_\ell - \mathrm{ahead}_\ell(k))$ at
iteration $\ell$, and a prefill of $S$ tokens admitted at iteration $\ell_0$,
where it received $g_0$ tokens from the budget left at its admission,
completes at the first $\ell$ with
$g_0 + \sum_{\ell_0 < \ell' \le \ell} \min(\mathrm{rem}_{\ell'}(k),\ B - d_{\ell'} - \mathrm{ahead}_{\ell'}(k)) \ge S$;
iteration $\ell'$ lasts $\tau(\text{batch}_{\ell'})$.

*Proof.* `Fill.assign_eq_fillIter` (no growing job, `engineQueuesEmpty`,
`Fill.lean:228`) and `assign_iter_eq_fillIter` (growing jobs whose
allocation covers the position they reach, distinct owners, nobody
waiting, `Fill.lean:154`): the iteration's assignment *is* `fillIter`,
which serves in order (`fillAmounts_fifo`), gives no job more than it
wants (`fillAmounts_le_want`) and hands out the budget or all that is
wanted (`fillAmounts_sum`). In admission order without a cap the fill is
decode-first, so the $d_\ell$ decoders take one token each before any
prefill: `Serve.serve_eq_decode_first`, which is a theorem about an
abstraction (`List ℕ` of remaining works, `Serve.lean:34`) not yet
connected to `Exec.assign` by a theorem, so this step is argued, not
proved end to end. The iteration $\ell_0$ of the admission itself has a
non-empty queue and is outside both `Fill` theorems, which is why its
contribution enters as the datum $g_0$.

What is proved is the service of one prefill while no request joins the
pools the stage admits for (the hypothesis `engineQueuesEmpty` at every
$\ell' > \ell_0$). The reading of the stage as a discrete-time FIFO queue with
arrivals, whose service capacity per slot is $B - d_\ell$ tokens and whose
slot length is $\tau(\text{batch}_\ell)$, both functions of the state, is
the natural extension and is a conjecture. The paper models the prefill
stage as "a FIFO queue whose server is available a fraction $1 - \rho_D$
of the time" (`main.tex:318-320`, verbatim); identifying that server with
this queue at the constant per-slot capacity $(1 - \rho_D) B$ is this
document's reading. The decode-first order is the invariant `Shape`
(`Serve.lean:27-31`, decoders before prefills in the residents' list),
carried by `shape_append_prefill` under FIFO admission and
`serve_preserves_shape` under the fill, and broken by preemption, which
re-queues a resident; the paper's proof marker on its decode-first
sentence (`main.tex:317`, `\provedby{serve_preserves_shape, …}`) inherits
the same `List ℕ`-to-`Exec.assign` gap. The hypotheses are the
theorems': a chunk cap breaks the order (`chunk_cap_breaks_shape`), and
an iteration whose growth crosses a block boundary or that admits while
filling is not yet covered by `Fill.lean` (`Fill.lean:16-19`). `vllm.sq`'s
decodes grow `kv` by a block every `bs` tokens, so the proposition covers
its iterations between boundary crossings, not all of them.

**Proposition 4 (Markov structure).** (i) For every workload,
$(X(t))_{t \ge 0}$ is a Markov process on the configuration space, a Polish
space (a countable union of products of $\mathbb R^k$ with countable
discrete sets). (ii) If every environment clock is exponential and every
mark is drawn from a law depending only on the session's current
attributes (the `vllm.sq` workload: `poisson`, `~exp` think, `~bernoulli`
continuation, `~exp`/`~uniform` lengths), the environment clocks may be
dropped from $\mathcal E$ and the process remains Markov. (iii) If
moreover every non-exponential clock is integer-valued (`IterEnd` with
$\tau \equiv 1$, a `delay` or `fifo` run of integer work) and is set at
an integer time (so that its residual at an embedding instant is an
integer), the marks are integer-valued, and no transition
reads an absolute or elapsed time
(`now`, `waited`, `age`, `last` in a key, a guard or a header; an
`observe` may record one), then the chain $(X(n))_{n \in \mathbb N}$
sampled at the integer times — every `IterEnd` is one under
$\tau \equiv 1$, and a step stage that idles while a `gate` runs has no
iteration end to embed at — is a discrete-time Markov chain on a
countable space, once the configuration
is taken modulo its absolute times: $t$ is dropped, each event time is
replaced by its integer residual (what the next transition needs), and
the enqueue, release and last-token times by their order, which is all
the transition reads of them (LRU reads the release order).
The oracle programs (`examples/oracle/vllm_request.sq`: explicit sessions
at $t = 0$, an integer `gate` delay, values in $\mathbb N$, `cost 1`, `now`
read only by `observe first = now`) are this case.

*Proof.* (i) is Glynn's observation that the supplemented process (state
and clock readings) is Markov: $\mathcal E$ carries absolute times, so the
residuals are in $X$; the estimators $\hat\theta_s$, the one
history-dependent quantity an expression may read, are in $Q_s$; every
draw is consumed at the instant it is needed, and the unread tail of an
i.i.d. sequence is independent of the read prefix. (ii): the residual of
an exponential clock is exponential and
independent of the past, so dropping it loses nothing. (iii): at an
integer time every remaining clock is exponential (forgotten) or an
integer clock set at an integer time, whose residual is an integer; with
integer marks and the absolute times reduced to their
order, every component of Definition 2 ranges over a countable set, and
the transition does not read what was dropped. $\square$

Two cautions. $t$ is a component of $X$, so "time-homogeneous" holds
trivially; a program that reads `now` or `serial` (`branch (now > 100)`,
`choose j by ((serial + j) % ND)`) is homogeneous only in that sense, and
Proposition 5 excludes such programs. And (ii) does *not* make the state
process a continuous-time Markov chain: `IterEnd` fires at the
deterministic $\tau(\text{batch})$, not after an exponential holding time,
and the arrival instants are real. The object is a Markov process, not a
solved one; neither it nor the chain of (iii) is enumerable in practice,
since $\mathcal C_m$ is a permutation of the live entries, and
`docs/review.md` §4 reached the same verdict for PRISM and Storm.

**Proposition 5 (Regeneration; conjecture).** Let the program read none
of `now`, `serial`, `est_lambda`, `est_rho`, `est_wait` and `price`,
every pool be `evict lru` without `spill`, and let $X_\emptyset$ be the
configurations with $I = \emptyset$ and no pending event but the next
arrival. Let $x, x' \in X_\emptyset$ differ *only* in the entries of their
caches. Then for every $\xi$ the paths from $x$ and from $x'$ agree on
every session observation and every gauge (not on the pool report's
`evicted_entries`, `evicted_units` and `mean_cached`, which count the old
entries' removal). Hence, with the regeneration epochs taken as the
hitting times of $X_\emptyset$ under `poisson`, or as the arrivals that
find $X_\emptyset$ under `renewal`, if the successive epochs have finite
mean spacing and the cycle rewards have finite mean, the cycles are
i.i.d. and the report's time averages are renewal-reward limits whose
batch-means intervals estimate something that exists. (The estimators $\hat\theta_s$ are never reset, so they differ
between visits to $X_\emptyset$; a program that reads them has no i.i.d.
cycles, hence the exclusion.)

*Sketch.* At $x \in X_\emptyset$ every entry of every $\mathcal C_m$
belongs to an ended session (`end` keeps prefixes, `docs/language.md` §3),
so no future hold reads it (`cachedin` reads the session's own entry) and
every future entry has a later release time, hence a later rank under
`lru`. Consider an admission and let $D$ be its deficit computed from
the live content and the need alone, $U_m + |\mathcal C_m^{\text{live}}| + \text{need} - M_m$,
the same number on both paths since $x$ and $x'$ agree outside their
caches (entries are rounded down to blocks, and eviction stops as soon as
$U_m + |\mathcal C_m| + \text{need} \le M_m$, `makeRoom_room`). Each path
evicts its own old entries first, since they precede every live entry
under `lru`, and then faces $D^+$: if $D \le 0$ neither path touches a
live entry, and otherwise both evict the same live entries and the same
tail-first blocks of the last one. The old entries thus change the count
of evictions and nothing a session or a gauge reads; a `spill` would turn
each such eviction into a job, visible through `queue(s)`, hence the
hypothesis. Under `evict by` the claim needs the keys to put an older
entry first (`by (age)` ascending evicts the youngest first, so the
conjecture is for `lru`). For the renewal structure the residual arrival
clock at the regeneration epoch must be fresh: under `poisson` every
hitting time of $X_\emptyset$ will do; under `renewal` the epochs are the
arrivals that find $X_\emptyset$, where the gap just drawn is fresh; under
`closed(n)` the set $X_\emptyset$ is unreachable. The eviction step of
this sketch is a theorem: `Exec.makeRoom_dead_irrelevant`
(`lean/Serq/Regen.lean`) states that for a set of owners whose entries all
come strictly before every other entry, and with more fuel than cached
units, the live entries `makeRoom` leaves are exactly those it leaves on
the pool without the dead entries, and `makeRoom_fuel` that the fuel is
then irrelevant. What remains unproved is the lift from one eviction to
the path: that admission, release and the step engine read the dead
entries through `makeRoom` alone. $\square$

The closed system (`arrive closed(n)`) has a bounded population and, under
Proposition 4 (iii)'s hypotheses and quotient with bounded marks, a finite
state space; irreducibility is then the program's to establish. The
metastability note's exact passage times are computed on a projection
chosen by hand, not on this space
(`serving-queue-theory/research/metastability.md`).

**Three kinds of theorem.** Propositions 2–3 hold on every path; the
oracle theorems (`lean/Serq/Oracle.lean`) hold on one $\xi$ each; the
paper's propositions hold for a projection under hypotheses. A statement
of one kind is not evidence for another: one scenario's path does not
establish a process property, and a projection's formula does not hold
on a path.

## 4. The paper's model is a projection

The three numbers the paper works with are functionals of $X$:

| Paper | Functional of $X$ | serQ |
|---|---|---|
| $L_P$, turns in the prefill stage | sessions queued at `reqs`/`kv`, plus residents whose run is a `prefill` | `queued(reqs)`, `residents - decoders` at the `Budget` moment |
| $L_D$, turns in the decode batch | residents whose run is a `decode` | `decoders` |
| resident bytes | $U_{kv}$ | `used(kv)` |
| the hit rate $h$ | the fraction of admissions that consumed a reusable prefix | `observe hit = c > 0;` with `c = min(cached, reusable(known, blocksize(kv)))` (`lib/vllm.sq:34-35`) |

The paper's propositions are about these projections under hypotheses
(Poisson arrivals to the prefill queue, a PS decode station, a fixed $h$).
Three facts about $X$ make the projection non-Markov; the paper states
the second and third as approximations (`main.tex`, §2.1–2.2) and takes
the first, a fixed $h$, as an input the policy chooses (§2.3), and the
companion research notes measure each:

1. **The hit is a function of the path, through $\mathcal C_m$.** Whether
   $i$'s entry survives its think time depends on what was released and
   allocated meanwhile, that is on the rank of $i$'s entry in $\prec_m$,
   not on $(L_P, L_D)$. The measurements show this is the main effect, not
   a correction: in the long-context replay on the NPU testbed 92–100 % of
   the misses arrived to a queue, and in the block-level model that
   reproduces them (`memory_model.py`) 88–96 % of the non-hits had their
   whole prefix resident when they were sent and lost it *while waiting*
   (`serving-queue-theory/research/memory-model.md`). A
   fixed $h$ cannot say that; the feedback map
   $H(h) = \mathbb E_Z[\,\text{survival}(Z + W(h); h)\,]$ of
   `analytic-memory.md` is the first-order repair, a mean-field closure of
   exactly this coordinate.
2. **The prefill server's capacity is state-dependent.** Proposition 3 is
   the exact object; the paper's server has a constant availability
   $1 - \rho_D$, and the PK formula it feeds is not linear in the
   capacity.
3. **The loop is closed through the system.** A session's next arrival is
   its last completion plus a think time, so the arrival process to the
   engine is a function of the engine's own state. The paper's
   finite-source bound (`prop:finite`) is the Markovian case; the
   block-level model's counterfactual at a cap of 8 shows the sign a
   product-form model cannot produce: a *faster* decode gives *more*
   misses, because shorter cycles raise the turn rate on the busy rank
   (`memory-model.md`, counterfactuals; a model result, not a measurement).

The formulas are theorems *about projections* of $X$, true under their
hypotheses, and the hypotheses are statements about $X$ that a program
either satisfies or does not: `mg1.sq` is an M/G/1 (one `fifo(1)`, Poisson
arrivals, no feedback, decidable from the IR); `vllm.sq`'s prefill queue
is not, in the three ways above.

## 5. A production deployment as a composition

The deployments in `examples/` are products of five component processes on
one clock, coupled in two ways: a session moves from one component's queue
to another's (a *handoff*), and an expression in one component reads
another's observables (a *look*). The IR has one state table; the
decomposition is a reading of it.

**The client** $\mathsf{Cl}$. Per session a closed loop: `turn` draws the
marks `(n, o, more)` $\sim G(y, \cdot)$, the request is a handoff to the
gateway, `tool` is an environment clock. This is where $\xi_{\mathrm{wl}}$
and $\xi_{\mathrm{sess}}$ enter and nothing of vLLM is here
(`examples/multi-turn/vllm.sq`, the `workload` block). With a trace it is a
replayer: the marks and the think times are the file, the arrival instants
are `serial * spacing`, and $\mathsf{Cl}$ is deterministic.

**The gateway** $\mathsf{Gw}$. `choose j in ND by (…)` is a deterministic
function of a *snapshot* of the engines' observables at the instant the
session reaches it (`holders(D[j].kv) + queued(D[j].kv)`,
`cachedin(D[j].kv)`), then a handoff to the engine chosen. The program
reads $X$ exactly; a production router reads its own picture of the pods
(`docs/use-cases/pd.md` describes llm-d's prefix-cache estimate and load
scorers and cites their sources), and nothing makes that picture equal to
the pods' state at the instant. The difference is an observation channel,
possibly with a delay, the one component of the production structure the
language has no word for (§6).

**An engine** $\mathsf{E}_r$. A step stage with its pools: the chain of
Proposition 4 (iii), whose state is $(\sigma_i)_{i \text{ at } \mathsf{E}_r}$,
$P_{kv_r}$, $P_{reqs_r}$ and $Q_{E_r}$, and whose transition at `IterEnd`
is `schedule()`. The six oracle scenarios are six short paths of one
$\mathsf{E}_r$; the Lean fragment (`lean/Serq/Exec.lean`) is this
component and no other.

**A link** $\mathsf{L}$. `run a, b (w)` is a flow holding two `ps` stages
at once; its rate is a deterministic function of the set of flows present,
set by `share maxmin` or `bottleneck` ([Bandwidth sharing](bandwidth-sharing.md)).
A `transfer` is a handoff whose work is the KV's size and whose completion
advances the destination's position $p$ (`load`) and releases the source's
lease.

**The composition.** `examples/pd-disaggregation/llmd_nixl_pull.sq` is
$\mathsf{Cl} \parallel \mathsf{Gw} \parallel (\mathsf{E}^P_r)_{r < NP} \parallel (\mathsf{E}^D_r)_{r < ND} \parallel \mathsf{L}$,
and one request's path through it visits seven places (router, prefiller's
queue, prefiller's engine, leased, decoder's queue, parked for the read,
decoder's engine), which the paper's three phases do not cover: the session
is a program counter, not a phase. The coupling that is neither a handoff
nor a look is the **lease**: the prefiller's blocks stay allocated, neither
evictable nor a preemption victim, from the prefiller's last token to the
completion of the decoder's read, so an invariant of $\mathsf{E}^P_r$'s
pool depends on a clock of $\mathsf{L}$. Proposition 2 holds here (a
leased allocation is allocated); the invariant one would want next, "no
leased block is read after its release", is a statement about the product
and is not written.

One structure the examples do not have: **ranks in lockstep**. On the NPU
testbed, data-parallel ranks advance together: a decoding rank emits one
token per 512-token chunk of a peer's prefill (`memory-model.md`, probe
C), and the lockstep model that fits the probes gives the common step the
duration $\max_r \tau_r$ (`memory-model.md`, "Model family"; a modelling
choice, since the probes could not separate a maximum from a sum, "Result
of the pre-registered validation"). That is $N$ engines with
*one* iteration clock, and `stage E[N] : step` gives each member its own.
The product is expressible as a configuration; the shared clock is not
expressible as a program, and the note names it as the first link of the
causal chain behind the long-context misses.

## 6. What writing the definition down found

The document adds nothing to the IR. Writing Definitions 4–5 and
Propositions 1, 2 and 5 against the interpreter found six things the
language or the interpreter should say and did not. Four are rules of the
language now (2026-10-03; `tests/settle.rs`), with no IR change under
`docs/ir.md` §Stability; two need an IR node and stay candidates for an
issue with a Before/After.

1. **A loop that passes without blocking hung the interpreter**
   (Lemma 1), and the linker checked nothing about loops: `serq check`
   accepted `loop { set w = w + 1; }`. *Outcome:* the linker requires every
   path through a loop body to reach a `run` (a constant zero work does not
   count), a `hold` whose body does, or `end` (`Validator::lets_time_pass`),
   and the run time ends the run with an error naming the session when a
   session executes a million statements at one instant or the ready
   sessions are served a thousand times per live session without time
   passing (`STEPS_PER_INSTANT`, `READIES_PER_SESSION`). No program of the
   corpus is affected. Making a zero-work run a transition at $t$ was
   considered and rejected: `loop { run tool (0); }` would then transition
   infinitely often at one instant, the Zeno path of Definition 5, and the
   order of its completion against the same instant's admissions would move
   the LRU order (#249).
2. **Positivity of `cost` was unchecked** (Definition 5). *Outcome:* an
   iteration that schedules tokens at a cost of 0 is a run-time error
   (`start_iteration`); the step that only preempted may still cost 0, as
   `tests/pool_semantics.rs` requires and Lemma 1's counters bound. A link
   check on the constant term was rejected: `vllm.sq`'s `cost c0 + max(omega
   + …, tokens * a)` has `c0 = 0` and is positive through `omega`.
3. **Per-session random substreams** (Proposition 1). *Outcome:* a
   session's `init` and `turn` blocks read a stream seeded from (seed,
   serial, turn), its statements one seeded from (seed, serial), and the
   machine's draws the interpreter's streams (`substream`, `do_turn`, the
   `Sample` arm of `eval`); Proposition 1 (ii) is now the clean
   common-random-numbers statement, and `tests/settle.rs` holds it (0 of
   the (session, turn) pairs differ where 967 of 4 733 did). The price,
   paid: every number a seeded run prints moved. The tests compare against
   closed forms, confidence intervals and traces and did not; the figures
   of `docs/` that quote seeded runs (`docs/validation.md`,
   `docs/language.md` §5) are measurements of their date, and the
   companion repository's serQ-derived numbers move when it moves its pin.
   The IR's version does not move: how the interpreter draws is its
   sampling, not the program's meaning (`docs/ir.md` §Stability).
4. **An observation channel.** $\mathsf{Gw}$ reads $X$ at the instant; the
   router reads a delayed or periodic picture. Today a stale router is
   written by hand, as session attributes. *Sketch, not compiled:* a
   `probe` is a gauge sampled on a clock of its own and held between
   samples, which is what a scraped metric is; the program reads it where
   it would read the observable, and the period is the program's.

   ```
   // Before (examples/pd-disaggregation/llmd_nixl_pull.sq): the router reads the pods exactly
   choose j in ND by (holders(D[j].kv) + queued(D[j].kv));

   // After: the router reads a picture refreshed every second
   probe load[j in ND] = holders(D[j].kv) + queued(D[j].kv) every 1;
   choose j in ND by (load[j]);
   ```

   In the process a probe is one more component of $Q_s$'s kind, updated
   by a state-set clock of period $T$: still a GSMP, and $\hat\theta_s$ is
   the special case of an estimator refreshed at every job. It is an IR
   field that changes what the program does, so it opens the next version;
   a sweep over `every` is what says what the estimate costs.
5. **A shared iteration clock.** $N$ step stages whose iteration ends at
   $\max_r \tau_r$ (§5, lockstep). `stage E[N] : step` cannot say it.
   *Sketch, not compiled:* a step family declared `lockstep` starts its
   members' iterations together and ends them at the slowest; a member
   with nothing to do idles for the common step.

   ```
   // Before: N engines, each on its own clock
   stage E[N] : step { budget B; cost c_it + a * prefilled + b * attention; memory kv; }

   // After: N ranks, one clock
   stage E[N] : step { budget B; cost c_it + a * prefilled + b * attention; memory kv; } lockstep;
   ```

   One Boolean on `CStep`, a change of what the program does, hence a
   version; the lockstep model of `memory-model.md` is the measurement it
   would be checked against.
6. **The memory invariant as an assertion** (Proposition 2). The
   interpreter never checked $U_m + |\mathcal C_m| \le M_m$. *Outcome:* a
   debug assertion at the end of every $\mathsf{settle}$ checks it for
   every pool (`interp.rs`), so every test, oracle run and
   differential-test case checks the invariant for the commands Lean does
   not cover; the whole suite passes it.

And the rest of Proposition 5 in Lean: its eviction step is
`Exec.makeRoom_dead_irrelevant` (`lean/Serq/Regen.lean`); the lift to the
path over `Exec.step` is not written. One question about the model, not
the language: whether the linker should refuse a draw in `cost` (and in
`budget`, `chunk`, a `ps` capacity, a hold's units, a `cache` clause),
which it accepts today (§1). Every shipped `cost` is a fitted function,
so the device is deterministic in the model and its residual (MAPE
2.7–5.7 %) is lost; a `~` there would be an environment clock, still a
GSMP, and the Lean fragment would lose the engine's determinism. Step
coalescing ([IR v4](ir-v4.md) §6), which applies a run of identical
iterations at once, needs the determinism, so refusing the draw and
coalescing are one decision.

## Self-critique

**Why not write the chain and solve it.** Because the state space of
Definition 2 is not enumerable: $\mathcal C_m$ alone is a permutation of
the live entries. `docs/review.md` §4 reached the same verdict for PRISM
and Storm, and the metastability note shows what is possible when the
projection is chosen by hand — 12 341 states for $(h, m, c)$ at $N = 40$ —
and what it costs: a lumped chain whose stationary law is exact to
$3 \cdot 10^{-16}$ and whose recovery time is off by a factor of three. The
process is defined so that such projections can be stated as projections;
it is not a candidate for exact solution.

**Why not a fluid or piecewise-deterministic model.** A PDMP is the right
limit of the step engine: between environment events the state moves
deterministically (the iterations), and step coalescing ([IR v4](ir-v4.md)
§6) is the observation that the deterministic stretch has a closed form.
But the quantities the replays turned on are discrete and at the edge: a
pool of 51 blocks against 54, one preemption, the block boundary a growth
crosses. A fluid limit removes exactly those, and the long-context replay
is the case where they were the result. The fluid view is the extension
(`philosophy.md`: "the rest is a fluid extension"), not the definition.

**"Deterministic machine" is a modelling claim.** The real engine has
asynchronous scheduling, a tokenizer, HTTP, garbage collection; the A100
fit absorbs them into its constants `c0` and `c_it` (the latter is
Definition 5's $\tau_0$ for that program), and on the NPU testbed the
0.08–0.13 s by which the block-level model's hit TTFT falls short of the
observed one in the light short-context arms (`memory-model.md`, "the
frontend offset") is what that model did not absorb. The claim is that
*what the model carries* of the machine is deterministic, and that the
measurements that matter (first-token times, hit classes) are reproduced
by that part. It is checked, by the oracle and the replays, and it is
exactly as wide as those checks: a deployment whose scheduler is not a
function of its state (a router that load-balances by hashing a request id,
say) has a machine coordinate in $\xi$, which the language can write
(`branch with`, a drawn `choose` key via a `set`) and §1's shape does not
describe.

**What is not given.** No comparison or monotonicity result: adding a block
to the pool can produce more misses (`memory-model.md`, sensitivity of the
block-level model `memory_model.py`, not a serQ run: the uncapped s20 arm
has 162 misses at 51 blocks and 163 at 52, the cap-16 arm 122 at 49 and
124 at 50), and the process says why (the closed loop, and
an LRU order one more block shifts), not that it cannot happen. No closed
form. Proposition 5 is a conjecture with the hypotheses its sketch needed
(`lru`, no `spill`, no `now`/`serial` and no online estimator read,
regeneration at hitting times under `poisson` or at the arrivals that find
the system empty under `renewal`); its eviction step is proved
(`makeRoom_dead_irrelevant`), its path lift is not. Proposition 3 covers the
iterations `Fill.lean` covers and takes the admission iteration as a datum.
Proposition 1 (iii) is path-wise only until the session's own path
diverges; (ii) no longer is.

**Why a design document and not a page of the spec.** `docs/language.md`
§3 is the semantics rule by rule, checkable one at a time against the
interpreter and the Lean fragment; this document is a reading of it as one
object, and its claims (Propositions 3–5, the three projection gaps, the
shape) are about the whole and are checked by the companion's measurements
and by theorems partly written. When Proposition 5 is proved the paragraph
that states it moves to the spec; until then the reading stays here, next
to the philosophy it makes precise.
