# Claims

A serQ program can now state propositions about its own paths, and Lean can
prove them about the same program. Three papers' propositions are written
this way (#257, #258, #260): the paper's serving system is a program, the
paper's proposition is a `claim` of it, and the proof is a Lean theorem
about every path of that program. This document records the construct, its
semantics in the interpreter and in Lean, what it costs, and what was
rejected.

## The case

The three papers' issues each named a proof target and a gap. The target
was a theorem *about a serving system*, Dai et al.'s "every work-conserving
scheduler fills its batch", Bari et al.'s "RAD fills its tiles", Kong et
al.'s "SVF's waiting is bounded by the volumes ahead of it". The gap was
that serQ could write the system and not the theorem. Lean could state a
theorem about `Exec.run`, but the statement was a Lean term written by
hand. Nothing tied it to the program a reader sees, and nothing failed when
the program changed.

## The construct

```
claim NAME [given ( expr )] : every iteration of STAGE ( expr ) ;
claim NAME [given ( expr )] : some iteration of STAGE ( expr ) ;
claim NAME [given ( expr )] : at end ( expr ) ;
```

- **`every iteration of S (e)`**: `e` holds at the start of every iteration
  of the step stage `S`, after its batch is formed (the moment the cost is
  read). It reads the cost's variables, `now`, the pool and stage
  observables, and two new ones. `demand` is what the residents could take
  with an unlimited budget, Dai et al.'s $\sum_i (p_i + \mathbf 1\{p_i=0\})$.
  `served` is the tokens of the earlier iterations.
- **`some iteration of S (e)`**: some iteration of some path. A refutation,
  such as FasterTransformer not being work-conserving.
- **`at end (e)`**: when every session has ended, over aggregates of the
  observations: `total`, `count`, `largest`, `smallest` and `prefix_total`,
  the sum of the sorted prefix sums, which is the lower bound of scheduling
  theory.
- **`given (g)`**: the paths whose every session satisfies `g` after
  `init`, the paper's hypotheses on the workload.

The interpreter checks each claim on the path it runs and reports holds,
fails at $t$, witnessed at $t$, not evaluated, or out of scope. A claim
reads and does not act: `Program.claims` is an added IR field a reader may
ignore. Under `docs/ir.md` §Stability that bumps nothing, and IR 11 is
untagged, so it goes in the coming tag's message.

## The meaning in Lean

`scripts/gen_lean_claims.py` reads the IR of each program under
`examples/papers/` and writes `lean/Serq/Claims.lean`: the program after
the workload's arrival delay and `init`, the deployment, the *family* of
workloads, and one `Prop` per claim, as defined in `Serq/Claim.lean`:

| Claim | Lean |
|---|---|
| `every iteration of S (e)` | `EveryIteration D W P q`: for every `w ∈ W` and every machine `m` with `Reach D w P m`, if an iteration runs, its record `m.last` satisfies `q` |
| `some iteration of S (e)` | `SomeIteration D W P q`: some `w ∈ W` and some reached machine whose running iteration satisfies `q` |
| `at end (e)` | `AtEnd D W P q`: every reached machine at which every session has ended satisfies `q` |

`Reach` is the machines a run reaches one event at a time, from
`Exec.start`. The family is the workload's support, widened where that is
sound. There are up to 500 sessions. An attribute `init` draws is any
natural number, and the arrival times under `poisson` are any. A constant
`renewal` gives the program's times, and `given` restricts every session.
Widening proves more, so it is sound for `every` and `at end`. A `some`
claim needs the support itself, so the generator refuses one whose workload
draws. The bound of 500 is where the fragment's fuel (`admitHeads`,
`settleLoop`, `drain`) is shown to suffice.

`lean/Serq/ClaimsProved.lean` is also generated: one line per claim,
`example : Claims.P.c := Papers.P.c`. The build fails when a claim has no
proof, or when the program now claims something its proof does not prove.

The fragment grew three things for the papers, each also in the
interpreter already:

- `serve only` in `Exec.assign`: a resident the predicate excludes takes no
  token (`Deployment.only`);
- `queue by` in `Exec.admitHeads`: the waiting session of least key, ties in
  queue order (`PoolDef.key`, `argminKey`);
- an iteration record, `Machine.last` and `Machine.served`, which is what a
  claim over iterations reads;
- costs with terms `k * ceil(tokens / b)` in the generator.

`Exec.admit`'s fold was named (`admitPool`) and `Exec.step` split into
`handle` and `afterEvent`, so that proofs can name them. The oracle theorems
check the meaning did not change.

## What every program satisfies

Three results hold for every program, not only the papers':

- **The engine's token rate** (`Exec.served_rate`). If every batch within
  the budget lasts at least `T/R` per token, then before every iteration the
  engine has served at most `R/T` tokens per unit of time. That is Dai et
  al.'s Theorem 2(a) and Bari et al.'s Theorem 1 for every scheduler serQ
  can write.
- **Sub-programs** (`Exec.reach_sub`). Every session runs a part of its
  program, and every job comes from one of its `run` statements. A proof
  reads the program text through this: a program without `growing` grows
  no hold (`jobs_not_growing`).
- **Work conservation** (`Exec.work_conserving`). On an engine that serves
  every resident and admits for no pool, every iteration takes
  `min budget demand`. It is `Fill.assign_eq_fillIter` lifted to every
  iteration of every path by `every_iteration_of`.

## The papers

| Paper | Claim | Proof |
|---|---|---|
| Dai et al. (#257) | `token_rate`, `work_conserving` (Sarathi), `not_work_conserving` (FasterTransformer) | `served_rate`; `work_conserving`; a witness of two requests, computed in the kernel |
| Bari et al. (#260) | `token_rate`, `optimal_tiling` (RAD) | `served_rate`; an invariant of RAD's run: prefill work stays a multiple of the tile |
| Kong et al. (#258) | `queueing_bound` (SVF), and Theorem 3.2 from it | an invariant of SVF's run: Lemma A.2's potential, kept by every operation; Proposition 3.1 for every feasible schedule |

[Dai et al.](../use-cases/dai.md), [Bari et al.](../use-cases/bari.md) and
[Kong et al.](../use-cases/kong.md) give each paper, its system, its
propositions and the proofs.

## The price

- **IR**: one field, `Program.claims`. One `CExpr` variant (`Agg`) and two
  `CtxVar` variants (`Demand`, `Served`). Three moments (`Given`,
  `Iteration`, `End`). No program without claims changes, and no oracle IR
  file changes.
- **Language**: six keywords (`claim`, `every`, `some`, `iteration`, `of`,
  `given`) and five aggregate names.
- **Lean**: `Exec` gains the fields above, and the oracle theorems,
  regressions and bench are unchanged. The proofs are about 4 800 lines:
  `Claim`, `Inv`, `Work`, and `Papers/*`.

## Self-critique

- **A claim as a Lean term in the program** (`claim c : lean "…"`). Rejected.
  The proposition would be written in Lean, not in serQ. A reader of the
  program could not read it, and the interpreter could not check it.
- **Temporal operators** (`always`, `eventually`, `until`). Rejected for
  now. The three papers needed a state predicate at iterations, an
  existential, and a statement at the end. A general temporal logic is a
  language of its own, and nothing asked for it.
- **`prefix_total` as a pair sum**, `sum i, j (min(vol[i], vol[j]))`.
  Rejected. It reads as arithmetic, not as the lower bound it is. It needs
  session-indexed attributes, which serQ does not have, and it is quadratic
  to write out.
- **Proving the papers about their own models.** Each paper's Markov chain
  or integer program, proved in Lean, would not be about the serving system
  a reader runs. Only Proposition 3.1 is about a model rather than the
  program, because OPT is a schedule no program runs.
- **Universal claims over the fragment's whole domain.** The families stop
  at 500 sessions, where the fuel of the executable semantics is shown to
  suffice. A run beyond it can exhaust the fuel and leave the serQ
  semantics, so the claims say nothing there.
- **`prefixTotal` sorted the wrong way.** The first `Exec.prefixTotal`
  reversed the sorted list, so it summed the *largest* prefixes. The
  interpreter's `prefix_total` was right. The disagreement went unnoticed
  until the mathematics was proved against the definition (`KongMath`), and
  Proposition 3.1 is false for the reversed form. Why it happened: the
  definition followed the name ("prefix sums of the ascending list"), and
  `prefixSums` adds suffix sums. Could a check have refused it? The Lean
  model and the interpreter now agree on one case by a theorem
  (`Exec.prefixTotal [3, 1, 2] = 10`, `Papers/Kong.lean`), and
  `tests/claims.rs` checks the interpreter on the same case.
