# The Lean model serves by keys

An engine's `advance running by (k₁, …)` (and `decode first`, which is
`by (decoding ? 0 : 1)`) lowers to the step's `serve` `By(keys)`: the
residents in ascending keys, ties in admission order. The interpreter has
served that way since the first `serve by`, and #322 made its iteration
follow vLLM where an order meets `preempt lifo`. The Lean model's
`Exec.assign` served in admission order only, and both Lean generators
refused a `serve` with keys. This change teaches the model and the
generators the keys, with #322's three rules, in one step (#328).

## The finding

Under `serve by`, the victim of a growth (the latest admitted resident,
`preempt lifo`) need not be the last served: it can be one the iteration
has served already, and the grower can be the victim while other
residents wait to be served. #322 made the interpreter do what vLLM's
PRIORITY path does there (`src/engine/interp.rs`, `give` and
`start_iteration`):

1. a served resident's computed position advances when the iteration is
   settled, for the residents still in it, not when it is served (vLLM
   `_update_after_schedule`, scheduler.py:1584-1597);
2. a served resident a growth preempts leaves the batch and its tokens
   return to the budget (scheduler.py:779-797);
3. a growth that preempts the grower itself ends the iteration's serving
   (scheduler.py:807-813, `break`).

`Exec.assign` did none of the three. It advanced a hold's position as it
served the job, went on serving at the same index after the grower
preempted itself, and `preemptVictim` took a served victim's tokens out of
`iter` without giving them back to `left`. No theorem reached the path:
the generators refused keyed `serve` (`gen_lean_oracle.py` and, until
#730, a second copy in `gen_lean_claims.py` that tested a value no IR had
held since #50), so every Lean program served in admission order. There
the victim is the last resident and has not been served, and a grower
that preempts itself is the last resident, so the three rules change no
answer. That the rules had to move with the refusal was written nowhere
next to the refusal.

The vLLM records of keyed orders existed since #350:
`tools/oracle/serve/{admission,newest_first,newest_first_preempt,prefill_first}`,
answered by vLLM's scheduler patched to serve in the program's order
(`tools/serq_vllm.py`), and kept apart from `tools/oracle/*.json` because
the fragment refused them.

## The change

`Exec.Deployment` gains a last field, `serveBy : List (ServeEnv → ℤ)`, the
keys. `assign` takes the jobs it has still to consider, `todo`, in place of
an index: `pick` returns the first job of `todo` with the least keys
(lexicographic, `keyLt`), read on the residents as they stand at that
pick, and the job is erased from `todo`. Without keys `pick` is the head
(`pick_of_nil`). This is the interpreter's set of the served with the
order re-read on every turn (`serving_order`, `order_by`): an index could
not survive the removal of a served victim placed before it, which is
rule 2's case. After an admission `todo` is the newcomer's job, which is
last in the list since it was admitted last.

The three rules:

1. `assign` records the tokens only; `settleIter` advances each growing
   job's hold by its tokens for the entries still in the batch, and
   `plan D m := settleIter (assign …)` is what `startIteration` runs.
   `iterStats` reads the positions after settlement, as before.
2. After a growth, `left` gains `tokSum m.iter - tokSum m'.iter`, the
   tokens of the entries the preemption removed, and `todo` keeps only
   the jobs still resident.
3. A growth that fails (the grower preempted itself) returns the machine:
   no one else is served and no one admitted.

The generators share one translation (`gen_lean_oracle.py`):
`fragment_step` accepts `By(keys)` and refuses `ExclusivePrefill` (its
prefill displaces the decodes served before it and takes the whole budget,
an iteration `assign` does not build); `serve_by` writes each key as
`fun e => ((N : ℕ) : ℤ)` or, for a negation of the whole key,
`fun e => -((N : ℕ) : ℤ)`, `N` the natural translation over `serve_leaf`
(moved here from the claims generator, which now imports it). Refused: a
key reading `Kvb`, `Kvp` or `Now` (`ServeEnv` does not carry them), a
negation inside a key, and a difference. With the sign only at the top,
one key's values are all `+N` or all `-N`, so the interpreter never
compares `-0.0` with `+0.0` (`total_cmp` orders them, ℤ does not) on it.

## Evidence

- `Oracle.vllm_serve_{admission,newest_first,newest_first_preempt,prefill_first}`:
  the four scenarios of `tools/oracle/serve/` run `vllmRequest`, the same
  translated program as the stock scenarios (`program` checks it), and give
  the patched scheduler's first and last steps and preemptions.
- `Regress.regress_served_victim` (`tests/lean-regress/served_victim.sq`):
  rule 2. No serve scenario exercises it (`tests/serve_preemption.rs` says
  so of `newest_first_preempt`): admission needs budget left, so a
  decode-only batch has at most the budget's residents and the refund never
  binds. The program serves newest first with a chunk of 12 out of 24
  tokens: at step 12 request 2 takes 12, request 0's chunk preempts it, and
  the 12 return, so request 1 decodes in that step (the interpreter's log:
  `"reqs": [[0], [1]], "each": [12, 1]`). The answer is the interpreter's.
- Each rule reverted in `Exec.lean`, `lake build Serq.Oracle Serq.Regress`
  fails on one theorem:

  | reverted | fails | the Lean model gives | the record |
  |---|---|---|---|
  | 1 (position advanced when served, no settlement) | `vllm_serve_newest_first_preempt` | first `(1, 25)`, `(3, 0)`, done `(1, 48)`, `(3, 22)` | first `(1, 26)`, `(3, 18)`, done `(1, 49)`, `(3, 23)` |
  | 2 (no refund) | `regress_served_victim` | done `(1, 28)` | done `(1, 27)` |
  | 3 (serving goes on after the grower preempts itself) | `vllm_serve_newest_first_preempt` | first `(1, 25)`, `(3, 17)`, done `(0, 25)`, `(1, 48)`, `(2, 15)`, `(3, 22)` | first `(1, 26)`, `(3, 18)`, done `(0, 26)`, `(1, 49)`, `(2, 16)`, `(3, 23)` |

- In admission order nothing moves: every earlier oracle and regression
  theorem holds unchanged, and `make drt` (random vLLM replay programs in
  admission order against the interpreter) passes.

## The proofs

The fill lemmas are about admission order by nature and gain
`hK : D.serveBy = []`, which the papers' deployments meet by `rfl`:
`assign_eq_fillIter_cov` (growing jobs whose holds cover their tokens),
`assign_eq_fillIter`, `assign_iter_eq_fillIter`, `assign_eq_fillIter_only`,
`assign_decode_first`, and the `plan` forms `plan_eq_fillIter` and
`plan_eq_fillIter_only`, which the papers use. `assign_iter_eq_fillIter`
and `assign_decode_first` lose their hypothesis that the jobs' owners are
distinct: it kept a hold moved at one job's serving from changing a later
job's, and `assign` no longer moves a hold as it serves. Work conservation holds in any order and keeps its
statement: `assign_tokSum` shows that `assign` hands out
`min left (Σ wants)` whichever job `pick` takes, and `work_conserving`
reads it. `assign_same` (the batch stays within the budget) carries the
refund: the batch plus the budget left is constant across a growth, since
the preemption removes from the one what the refund adds to the other.

## The price

No IR field, variant or meaning moves, so `IR_VERSION` does not; the
generators still pin 15. What moves is the Lean model's interface:

- `Exec.Deployment` has a last field, and an anonymous-constructor literal
  gains `, []` (`Kong.lean`, `DaiBounded.lean`, `lean/Bench/Main.lean`;
  the generated files);
- `assign`'s third argument is the job list (`m.jobs` where it was `0`);
  `startIteration` runs `plan`, and a proof that unfolds it rewrites with
  `plan_eq_fillIter` (one line in each of `Kong`, `DaiBounded`,
  `DaiStable`, `Bari`, `BariStable`);
- `tokSum` moves from `Claim.lean` to `Exec.lean`, and `serves` reads the
  factored `serveEnv`.

The companion `serving-queue-theory` calls `assign D 100000 {…} 0
D.budget` and `assign_iter_eq_fillIter` in its `StepEngine.lean`; when it
moves past the next tag it passes `(ms j).jobs` for `0` and `rfl` for `hK`.

## Self-critique

- *Roll the victim back in `preemptVictim`* (subtract its tokens from the
  position it was advanced to) would have kept `assign`'s shape and most
  proofs. Rejected: the rule becomes an undo written in a second place,
  and the position a preempted hold caches would be computed twice.
- *An order computed once per iteration, with the index kept.* Rejected:
  the interpreter re-reads the keys at every pick (an admission or a
  preemption moves the totals a key reads), and an index skips a job when
  a victim before it leaves.
- *A list of the owners served* in place of `todo`. Rejected: it needs
  owners to be distinct as a new hypothesis of every fill lemma's caller,
  and the invariant `work_conserving` runs on does not carry it.
- *A key with a descending flag.* Rejected: a second spelling of a
  negation the IR already writes as `Unary(Neg, …)`.
- *A ℤ translation of every expression.* Rejected: it would duplicate the
  trusted ℕ translator for one sign; the top-level negation is all the
  scenarios and `preempt lifo`'s spelling need.
- *`hK` on `work_conserving` too.* Rejected: the theorem is true in any
  order, and restricting it would leave a `decode first` claims program
  without it for no reason.
- Not done: random differential testing over serve orders (`lean_drt.py`
  generates the vLLM replay program, and `Bench` reads no keys), and a
  vLLM record of the refund (the machine that writes one needs vLLM at the
  pin; the regression's answer is the interpreter's).
