# Engines on devices

A step engine is a scheduler on a device. The device says what hardware
there is; the engine holds its slots, schedules a batch each iteration and
executes it; a pool is one capacity, declared with the rules that hand it
out. All of it is parse-time sugar: no IR node, field or version is added.

## The problem

Today `examples/multi-turn/vllm.sq` writes `pool kv { cap …; }`,
`pool reqs { cap max_seqs; admit via engine; }` and `stage engine : step
{ budget B; chunk long_prefill(reqs, c); cost c0 + max(omega + beta *
(kv_decode + kv_prefill), tokens * a); memory kv; }`. A reader cannot see:
which resources the cost's two arms are (HBM, compute); which limits are
hardware (`kv`) and which the operator's (`reqs`, `budget`, `chunk`); that
`cost` is read after the batch is chosen and the rest before (#395); that
`budget` and `chunk` cap one quantity, `tokens`; the condition hidden in
`long_prefill`; where `residents`, `queued`, `preempted` and `admitted`
come from; and that with no `iteration` body the language runs vLLM's
`schedule()` (criterion 2, [engine neutrality](engine-neutrality.md)).

## The design

```
device gpu {
  compute (t) = t * a;               // time resource: a demand to time, inlined
  hbm (k)     = omega + beta * k;
  kv cap blocks * bs;                // capacity
}
engine engine on gpu {
  reqs cap max_seqs;                 // max_num_seqs: a capacity the engine holds
  tokens cap B;                      // max_num_batched_tokens: the tokens one iteration computes
  schedule {
    let cap = running.count + waiting.count > 1 ? max_per_request : inf;
    advance running each at most (cap);
    branch (running.preempted == 0) {
      admit waiting each at most (cap);
    }
  }
  execute (c0 + max(hbm(kv_decode + kv_prefill), compute(tokens)));
}
pool kv on gpu { block bs; evict lru; preempt lifo; }
pool reqs on engine { queue fifo; }
```

- **`device`** lists time resources (demand to time, read in `execute`;
  functions because the fitted constants are times) and capacities.
- **`pool X on D`** takes its name and `cap` from D's capacity; its body is
  the rules (`block`, `evict`, `preempt`, `queue`). A pool on an engine or on
  its device is admitted by that engine, so `admit via` and `memory` go: the
  engine's KV is the pool on its device. A family follows its device.
- **`tokens cap B`** caps `tokens`, which starts at 0 each iteration, is
  what `execute` reads, and is the language's name, so it cannot be a pool.
- **`schedule`** is required and is vLLM's `schedule()` written out:
  `advance running` is its running loop (`scheduler.py:626-823`), `admit
  waiting` its waiting loop (`:872-1128`), skipped in a step that preempted
  (`:869`). Both spend `tokens`, in the order written; SGLang and TGI write
  other orders. `each at most (e)` caps one run's tokens, read per run as
  `only` is, with no comma and in one clause order (`only`, `by`/`while`,
  `each at most`); no cap is `inf`, not 0. A decode is not named: it has one
  token left (`scheduler.py:670-678`). `let` stands first and is read at the start.
- **What a schedule reads** is named after its list: `running.count`
  (`residents`), `running.decoding` (`decoders`), `running.preempted`,
  `waiting.count` (every queue the engine admits, as vLLM counts
  `skipped_waiting`, `scheduler.py:609-611`), `waiting.admitted`.
- **`execute (T)`** is the batch's time over the device's resources; a
  serial engine writes `+` for `max`. `cost` is only a request's work.

Link errors, each with its reason: a pool on no such capacity, or with
`cap` or `admit via`; an undeclared capacity; two pools or engines on one
device; a time resource outside `execute`; no `tokens cap`; a constant cap
≤ 0; a `let` not first or read outside `each at most`; two `each at most`
that differ.

## Lowering

`tokens cap` is `budget`; `each at most (c)` is `chunk` with `inf` written
0; `advance running`/`admit waiting` are `serve`/`admit`; the list values
are the context variables, `waiting.count` the sum of `queued(p)`;
`execute` is `cost`; the engine is a step stage with `memory` the device's
pool. vLLM's body lowers to no `iteration`, as the Lean fragment
(`gen_lean_oracle.py`'s `only_body`) and the oracle read it: written out,
its IR would differ. IR values move in two places with the same meaning: a
device pool gains `admit_via` (the 12 programs without it have no hold in
`kv`'s queue), and `waiting.count` adds an empty `queued(kv)`. The oracle
IR, the Lean theorems and `lean_bench.py`'s `chunk_of` move with them.
Naming the resources is checked: `def compute`/`def hbm` in the cost give
byte-identical IR. The 21 step programs are rewritten and the old spellings
refused with the new one ([one admission](one-admission.md)), as a stack:
`device`, then `engine`, then `schedule` and `execute`.

## Self-critique

- **Split the IR (#395)**: its halves are used one to one. A batch handed to
  a `fifo(c)` or a `step { schedule; execute; }` waits for vLLM's batch queue
  (`engine/core.py:214-220, 673-729`, `config/vllm.py:592-602`).
- **`per request`, `reqs.items`, `decode at most (1)`, `chunk`**: `request` is
  a retired keyword; vLLM caps every run alike, a decode has one token left.
- **`serve`/`admit`, `admit; start waiting`, budget on `admit` only**: `serve`
  names a queue's service; admitting and first tokens are one act
  (`scheduler.py:1078-1128`); both loops spend the budget.
- **`let p = advance running`, a default schedule, an `admit` method,
  `memory` from `growing`**: SGLang advances twice; the others restore a rule.
- **Open**: a run's own attributes, `batch.kv_decode`, the queue a hold
  waits in, `exclusive prefill`, `granule`, the name `stage`.
