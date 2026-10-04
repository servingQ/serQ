# Serving a subset

`serve only (p)` says which residents a step iteration serves. `serve by`
already says in what order. It is a field of `CStep` beside `serve`, added
while IR 11 is untagged (#261).

## The case

A step stage's `serve` was an order (`by`) or the rule that a prefill runs
alone (`exclusive prefill`). Two of the schedulers that the optimal-scheduling
papers compare could not be written:

- FasterTransformer as Dai et al. model it (arXiv 2504.07347 §4, decode
  first, no mixed batching, #257);
- RAD (#260): prefills only in one mode, decodes only in the other, with the
  mode a function of the state.

Both exclude a class of residents depending on the state, and no `serve`
excluded anything but `exclusive prefill`, whose opposite could not be
written (design criterion 2). The grammar before:

```
[serve admission ; | serve by ( expr , ... ) ; | serve decode first ;
 | serve exclusive prefill ;]
```

## The construct

```
serve only ( expr ) [admission | by ( expr , ... ) | decode first] ;
```

- `p` is read at `Moment::Serve` for every resident, from the variables a
  serve key reads (`decoding`, `admission`, `remaining`, `residents`,
  `decoders`, `kv_decode`, `kv_prefill`, `now`), and may not draw.
- A resident `p` reads as 0 gets no token this iteration. It keeps what it
  holds and advances no computed KV, as a displaced decode does under
  `exclusive prefill`.
- The order that follows orders the rest; `admission` when none is written.
  `only` decides the set and `by` the order, and neither does the other's
  job (criterion 0).
- A session admitted through `admit via` that `p` excludes ends that
  iteration's admission.
- `only` does not combine with `exclusive prefill`.

FasterTransformer, and its opposite:

```
serve only (decoders > 0 ? decoding : !decoding);          // decodes alone while one decodes
serve only (decoders < residents ? !decoding : decoding);  // prefills alone while one is resident
```

The second is the resident half of `exclusive prefill`. The other half, a
waiting prefill that displaces the decodes, is an admission rule, which a
predicate over residents does not reach. That is why `exclusive prefill`
stays.

## The numbers

`examples/single-turn/fastertransformer.sq` is #257's point C program with
the first line above. At seed 1 and horizon 500 s, the mean of `pending` is
529.4 and its maximum 1059, against 95.3 and 101 for Sarathi. The decode
batch averages 1.0 and no batch is mixed. The other three schedulers'
numbers in #257 are unchanged. The [use case](../use-cases/fastertransformer.md)
has the table. `tests/serve_only.rs` checks the schedule on a unit clock.

## The price

- IR: one field, `CStep.only: Option<CExpr>`, omitted when absent, so every
  existing IR file is unchanged, the seven oracle files included. It changes
  what a program does, so on a tagged version it would have opened 12. 11
  has no tag, so it goes in the coming tag's message (`docs/ir.md`
  §Stability).
- Lean: `scripts/gen_lean_oracle.py` raises `Fragment` on a stage with
  `only`. The fragment serves every resident, and no oracle program uses
  `only`.
- Language: one keyword, `only`.

## Self-critique

- **`exclusive decode`, a second variant of `CServe`.** Rejected. It writes
  FasterTransformer and nothing else. RAD would ask for a third variant, and
  two variants cannot be combined: the two-booleans problem that `CServe`
  replaced in 4.
- **A budget per class (a decode budget of 0).** Rejected. `budget` is one
  number for an iteration, not a class. A per-class budget is two
  expressions where one predicate does the job.
- **`only` inside `CServe::By`.** Rejected. It would make `only` an order
  key, and `ExclusivePrefill` would still need its own. As a field of
  `CStep` it is beside the order, which is the relation the text says.
- **`only` with `exclusive prefill`.** Refused. The exclusive rule admits a
  waiting prefill in place of the decodes it displaces. A predicate that
  excludes the prefill, or the decode it displaced, would need a rule for
  which one wins, and nothing in a program has asked for one.
- **Admission past an excluded resident.** Without the rule that an excluded
  admission ends the iteration's admission, a stage would keep admitting
  requests that do not run, up to the pool's cap, since an excluded request
  takes no budget. The rule is the same as for a queue head that does not
  fit, which stops the iteration's admissions (`docs/language.md` §3). A
  program that wants to admit
  ahead of service can write a hold outside the stage's queue.
