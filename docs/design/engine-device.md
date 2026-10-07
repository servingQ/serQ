# Engines on devices

A step engine is a scheduler on a device. The device says what hardware
there is; the engine holds its slots, schedules a batch each iteration and
executes it; a pool is one capacity, declared with the rules that hand it
out. All of it is parse-time sugar: no IR node, field or version is added.

## The problem

Today `examples/multi-turn/vllm.sq` writes `pool kv { cap blocks * bs; … }`,
`pool reqs { cap max_seqs; admit via engine; }` and `stage engine : step
{ budget B; chunk long_prefill(reqs, chunk_cap); cost c0 + max(omega + beta
* (kv_decode + kv_prefill), tokens * a); memory kv; }`. A reader cannot
see: which resources the cost's two arms are (HBM, compute); which limits
are hardware (`kv`) and which the operator's (`reqs`, `budget`, `chunk`);
that `cost` is read after the batch is chosen and the rest before (#395);
that `budget` and `chunk` cap one quantity, `tokens`; the condition hidden
in `long_prefill`; where `residents`, `queued`, `preempted` and `admitted`
come from; and that with no `iteration` body the language runs vLLM's
`schedule()` (criterion 2, [engine neutrality](engine-neutrality.md)).

## The design

```
let max_per_run = args.number("max_per_run", inf);   // long_prefill_token_threshold

device gpu {
  compute (t) = t * a;               // time resource: a demand to time, inlined
  hbm (k)     = omega + beta * k;
  kv cap blocks * bs;                // capacity
}
engine engine on gpu {
  reqs cap max_seqs;                 // max_num_seqs: a capacity the engine holds
  tokens cap B;                      // max_num_batched_tokens: the tokens one iteration computes
  schedule {
    let threshold = running.count + waiting.count > 1 ? max_per_run : inf;
    advance running each at most (threshold);
    admit waiting while (running.preempted == 0) each at most (threshold);
  }
  execute (c0 + max(hbm(kv_decode + kv_prefill), compute(tokens)));
}
pool kv on gpu { block bs; evict lru; preempt lifo; }
pool reqs on engine { queue fifo; }
```

- **`device`** lists time resources (demand to time, read in `execute`;
  functions because the fitted constants are times) and capacities.
- **`pool X on D`** takes its name and `cap` from D's capacity `X`; its body
  is the rules (`block`, `evict`, `preempt`, `queue`). A pool on an engine or
  on its device is admitted by that engine, so `admit via` and `memory` go:
  the engine's KV is the pool on its device. Under `device gpu[N]`,
  `pool kv on gpu` is `kv[N]` and `engine E[N] on gpu` pairs `E[i]` with
  `kv[i]`; no size is written twice.
- **`tokens cap B`** caps `tokens`, which starts at 0 each iteration, is
  what `execute` reads, and is the language's name, so it cannot be a pool.
- **`schedule`** is required and is vLLM's `schedule()` written out:
  `advance running` is its running loop (`scheduler.py:626-856`), `admit
  waiting` its waiting loop (`scheduler.py:872-1344`), which does not run in
  a step that preempted (`scheduler.py:869`). `while` is read before each
  admission, as the kernel reads it, because in serQ an admitted run grows
  and may preempt another under `preempt by (…)`. Both spend `tokens`, in
  the order written; SGLang and TGI write other orders. `each at most (e)`
  caps one run's tokens, evaluated once at the start of the iteration and
  applied to each run, with no comma and in one clause order (`only`,
  `by`/`while`, `each at most`); no cap is `inf`, not 0. A decode is not
  named: it has one token left (`scheduler.py:670-678`). `let` stands first
  and is read at the start.
- **What a schedule reads** is named after its list: `running.count`
  (`residents`), `running.decoding` (`decoders`), `running.preempted` (the
  runs preempted so far this iteration, a count), `waiting.count` (every
  queue the engine admits, as vLLM counts `skipped_waiting`,
  `scheduler.py:609-611`), `waiting.admitted`. The bare names are retired
  wherever these replace them (serve keys, claims, gauges), so one value
  has one spelling. The eviction key `waiting` (an entry whose session is
  queued, `src/frontend/link.rs:236`) would read as the list; it is a
  per-item attribute and is renamed with them (Open, below).
- **`execute (T)`** is the batch's time over the device's resources; a
  serial engine writes `+` for `max`. `cost` is only a request's work.

Link errors, each with its reason: a pool on no such capacity, or with
`cap` or `admit via`; an undeclared capacity; two pools on one capacity;
two engines on one device, or two pools on its device both able to be the
KV; a time resource outside `execute`; no `tokens cap`; a constant cap ≤ 0;
`inf` inside a cap's arithmetic rather than as a whole outcome; a `let` not
first or read outside `each at most`; two `each at most` that differ.

## Lowering

`tokens cap` is `budget`; `each at most (c)` is `chunk`, with an outcome
that is `inf` as a whole (all of `c`, or a branch of a `?:`) written 0;
`advance running`/`admit waiting` are `serve`/`admit`; the list values are
the context variables, `waiting.count` the sum of `queued(p)`; `execute` is
`cost`; the engine is a step stage with `memory` the device's pool. vLLM's
body, `advance running …; admit waiting while (running.preempted == 0) …;`,
is the kernel's default procedure statement for statement and lowers to no
`iteration`, which the oracle fragment requires (`deployment()` in
`scripts/gen_lean_oracle.py`).

IR values move in two places with the same meaning, and both are a
handshake, not a regeneration:

- **A device pool gains `admit_via`.** The 12 programs without it have no
  hold in `kv`'s queue. But the Lean fragment requires an engine-admitted
  pool to preempt nothing (`deployment()` again), and all eight
  oracle programs' `kv` is `lifo` without `admit_via`. That rule is relaxed,
  `Exec.lean`'s `viaEngine` case and `lean_bench.py` taught it, and `make
  lean` re-proved, before `pool kv on gpu` lands.
- **`waiting.count` adds an empty `queued(kv)`**; `lean_bench.py`'s
  `chunk_of` learns the sum.

Both go in the coming tag's message. Naming the resources is checked:
`def compute`/`def hbm` in the cost give byte-identical IR. The 21 step
programs are rewritten and the old spellings refused with the new one
([one admission](one-admission.md)), as a stack: `device`, then `engine`,
then `schedule` and `execute`. The count of rules moves the other way:
six go (`memory`, `admit via`, `budget`, `chunk`, a step's `cost`, the
default iteration) and about twelve forms and ten link errors come.

## Self-critique

- **Split the IR (#395)**: its halves are used one to one. A batch handed to
  a `fifo(c)`, or a `step { schedule; execute; }` around both, waits for
  vLLM's batch queue (`engine/core.py:214-220, 673-729`,
  `config/vllm.py:592-602`), where scheduling overlaps execution.
- **One `iterate` body**: position rules (budget first, cost last) for what
  two parts say by form.
- **A `schedule budget B, per request (c)` header, `tokens cap (c) per run`,
  `reqs.items`**: `request` is a retired keyword; the cap is on runs, not on
  `reqs`'s holders (TGI has no `reqs`); and its condition is decided each
  iteration from the lists, so it is logic for the body, not a declaration.
- **`tokens cap B, each at most (…)` on the engine**: one place, no `let`
  rules, but the same objection: the threshold reads the lists each
  iteration. Kept on the statements for now, with the kernel's one cap
  enforced by the "differ" error.
- **`decode at most (1)`, `chunk`, `prefill at most`, `execute at most`,
  `serve`/`admit`, bare `running`, `start running`**: vLLM caps every
  request alike and a decode has one token left; the others name a result,
  one mode, an action, a queue's service, no act, or runs not yet started.
- **`admit; start waiting`, or the budget on `admit` only**: admitting and
  the first tokens are one act (`scheduler.py:1263, 1296, 1316-1317`: pop,
  append to `running`, schedule its tokens, spend the budget), and both
  loops spend the budget.
- **`branch (running.preempted == 0) { admit … }`**: equal to the kernel's
  `while` only if admission cannot preempt, which holds under `lifo` and
  not under `preempt by (…)`.
- **`let p = advance running`**: SGLang advances twice; the kernel keeps the
  iteration's count, `running.preempted`.
- **A default schedule, an `admit` method, `admit from q`, `uses … overlap`,
  a `batch` block, `memory` from `growing`**: a rule back in the language, a
  second place for what is said, or (TGI) a pool that never grows.
- **Open**: names for a run's own attributes (`decoding`, `remaining`) and
  the eviction key's `waiting`; `batch.kv_decode` in `execute`; the queue a
  hold waits in; `exclusive prefill`; `granule`; the conventional stage name
  `engine` beside the keyword (`engine engine`); the name `stage`.
