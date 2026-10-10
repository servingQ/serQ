# Engines on devices

A step engine is a scheduler on a device. The device says what hardware
there is; the engine holds its slots, schedules a batch each iteration and
executes it; a pool is one capacity, declared with the rules that hand it
out. All of it is parse-time sugar: no IR node, field or version is added.

## The problem

`examples/multi-turn/vllm.sq` writes `stage engine : step { budget B;
chunk long_prefill(reqs, chunk_cap); cost c0 + max(omega + beta *
(kv_decode + kv_prefill), tokens * a); memory kv; }` beside `pool kv` and
`pool reqs { …; admit via engine; }`. A reader cannot see which resources
the cost's arms are; which limits are hardware and which the operator's;
that `cost` is read after the batch is chosen and the rest before (#395);
that `budget` and `chunk` cap one quantity, `tokens`; the condition inside
`long_prefill`; where `residents`, `queued`, `preempted` and `admitted`
come from; or that with no `iteration` body the language runs vLLM's
`schedule()` (criterion 2, [engine neutrality](engine-neutrality.md)).

## The design

```
let max_per_run = args.number("max_per_run", inf);   // long_prefill_token_threshold

device gpu {
  compute (t) = t * a;               // time resource: a demand to time, inlined
  hbm (k)     = omega + beta * k;
  kv cap blocks * bs;                // capacity
}
engine vllm on gpu {
  reqs cap max_seqs;                 // max_num_seqs: a capacity the engine holds
  tokens cap B;                      // max_num_batched_tokens: the tokens one iteration computes
  schedule {
    let threshold = running.count + waiting.count > 1 ? max_per_run : inf;
    advance running each at most (threshold);
    admit waiting while (running.preempted == 0) each at most (threshold);
  }
  execute (c0 + max(hbm(batch.kv_decode + batch.kv_prefill), compute(batch.tokens)));
}
pool kv on gpu { block bs; evict lru; preempt lifo; }
pool reqs on vllm { queue fifo; }
```

- **`device`**: time resources (demand to time, read in `execute`; functions
  because the fitted constants are times) and capacities.
- **`pool X on D`** takes its name and `cap` from D's capacity `X`; its body
  is the rules. The engine's KV is the pool on its device, so `memory` goes.
  Who admits a hold waiting for the pool is said where the pool is:
  `pool reqs on E` and `pool kv on E.D` are admitted by engine `E` in an
  iteration, `pool kv on D` as soon as it fits; so `admit via` goes. Under `device gpu[N]`, `pool kv on gpu` is `kv[N]` and
  `engine E[N] on gpu` pairs `E[i]` with `kv[i]`.
- **`tokens cap B`** caps `tokens`, which starts at 0 each iteration and is
  what `execute` reads; a language name, so never a pool.
- **`schedule`** is required: vLLM's running loop (`advance running`,
  `scheduler.py:626-856`), then its waiting loop (`admit waiting`,
  `scheduler.py:872-1344`), which does not run in a step that preempted
  (`scheduler.py:869`). `while` is read before each admission, as the kernel
  reads it: in serQ an admitted run grows and may preempt another under
  `preempt by`. The order is the body's; SGLang and TGI write others.
  `each at most (e)` caps one run's tokens, evaluated once as the iteration
  starts and applied to each run; clauses take no comma and one order
  (`only`, `by`/`while`, `each at most`); no cap is `inf`, not 0. A decode
  is not named: it has one token left (`scheduler.py:670-678`).
- **Values** are named after their list: `running.count` (`residents`),
  `running.decoding` (`decoders`), `running.preempted` (a count this
  iteration), `waiting.count` (every queue the engine admits, as vLLM counts
  `skipped_waiting`, `scheduler.py:609-611`), `waiting.admitted`, and the
  residents' KV `running.kv_decode`, `running.kv_prefill`. The batch's are
  `batch.tokens`, `batch.prefilled` (in `schedule`, what it has formed so
  far) and `batch.decoding`, `batch.kv_decode`, `batch.kv_prefill`,
  `batch.attention` (in `execute`). The kernel reads `decoders` and
  `kv_decode` as the residents' before the batch is formed and as the
  batch's in `cost`, and a budget or an `only` can leave a resident out of
  the batch, so the engine names them twice (#416). An engine reads no bare
  name in its `tokens cap`, `execute` and `schedule`; `running.preempted`
  and `waiting.admitted` say what the schedule did, so only `schedule`
  reads them.
- **`execute (T)`**: the batch's time over the device's resources; a serial
  engine writes `+` for `max`. `cost` is only a request's work.
- **Names.** `engine` is a keyword, so an engine is named for what it
  models (`vllm`, `sglang`, `tgi`) or `llm`, as `docs/writing-programs.md`
  names it. Inside a `queue`, `device gpu` is the member's (`Q.gpu`, a
  family as the queue is) and `engine on gpu` is the queue's stage, named
  after it as the queue's `serve` is, so `pool reqs on Q` reads "the
  requests `Q` admits". A queue's pool is on its own device or on `Q`,
  never on a name outside the queue, and a queue's device takes the names a
  top-level one may.

Link errors, each with its reason: a pool on no such capacity, or with
`cap` or `admit via`; an undeclared capacity; a capacity declared as a pool
twice; two engines, or two candidate KV pools, on one device; a time
resource outside `execute`, or a `def` named like one; no `tokens cap`, or
a constant one ≤ 0; an `each at most` that chooses anything but constants
(its condition may read the iteration), or one ≤ 0 once the `let`s are
known, or `inf` inside its arithmetic; `[N]` on a pool `on` a family; a
`let` not first or read outside `each at most`; two `each at most` that
differ; a family engine's `waiting.count` (no member may count every
member's queues); a queue named `running` or `waiting`; a queue's pool on
a device or an engine outside the queue, a top-level pool on a queue's
engine, a queue that holds an engine named for a keyword, a queue's
device named as the queue (`on Q` would be two things), and a queue, with
a stage or without, named as a top-level device or engine, in either order. A capacity's `cap`
is checked as a pool's is.

## Lowering

`tokens cap` is `budget`; `each at most (c)` is `chunk`; `advance
running`/`admit waiting` are `serve`/`admit`; list values are the context
variables; `execute` is `cost`; the engine is a step stage whose `memory` is
its device's pool. vLLM's body is the kernel's default procedure statement
for statement and lowers to no `iteration`, as the oracle fragment requires
(`deployment()` in `scripts/gen_lean_oracle.py`); `advance running only
(p); admit waiting only (p) while …` lowers to `serve only (p)`, and
`exclusive prefill; admit waiting while …` to `serve exclusive prefill`.

No IR value moves, so the oracle and the Lean fragment read every program
as before:

- **`admit_via` as the pool says.** A pool on an engine, or on its device
  as `on E.D`, is admitted by it; `on D` is not. A program that held the
  device pool first with no `admit via` (the tests' queues, settle-time
  admission) and one that did (llm-d's decoder, TGI) are both written.
- **`waiting.count`** is `queued(p)` summed over the pools whose `admit_via`
  is the engine, which is `queued(reqs)` where `long_prefill` read it.
- **`inf` is written 0 by the linker**, once the `let`s have their values:
  an outcome of `inf` (the whole cap, or a branch of a `?:`) is no cap,
  which the kernel writes 0 as `min(remaining, inf)` gives. The linker does
  it for every step, so an old `chunk inf` is 0 too. An engine's outcomes
  are constants once linked, so one at or below 0, which the kernel would
  read as no cap, does not link: the program never means one thing and runs
  another.

`tests/engine_device.rs` holds `vllm.sq`, `sglang.sq` and `tgi.sq`, engines,
to the IR of their step stages, byte for byte, as it holds each stage form
to its schedule. The 21 step programs are rewritten and the old spellings
refused with the new one ([one admission](one-admission.md)), as a stack:
the forms, the programs, the documentation, then the refusal. Six rules go
(`memory`, `admit via`, `budget`, `chunk`, a step's `cost`, the default
iteration); about twelve forms and ten link errors come.

## Self-critique

- **Split the IR (#395)**: the halves are used one to one. A batch handed to
  a `fifo(c)`, or `step { schedule; execute; }`, waits for vLLM's batch
  queue (`engine/core.py:214-220, 673-729`, `config/vllm.py:592-602`).
- **One `iterate` body**: position rules for what two parts say by form.
- **The per-run cap in a header (`per request`), on the engine (`per run`,
  `tokens cap B, each at most`) or on `reqs.items`**: `request` is a retired
  keyword; TGI has runs and no `reqs`; the threshold reads the lists each
  iteration, so it is the body's. The "differ" error keeps one cap.
- **`decode at most (1)`, `chunk`, `prefill at most`, `execute at most`,
  `serve`/`admit`, bare `running`, `start running`**: a decode has one token
  left; the others name a result, one mode, an action, a queue's service,
  no act, or runs not yet started.
- **`admit; start waiting`, the budget on `admit` only**: admitting and the
  first tokens are one act (`scheduler.py:1263, 1296, 1316-1317`), and both
  loops spend the budget.
- **`branch (running.preempted == 0) { admit … }`, `let p = advance
  running`**: the branch equals `while` only under `lifo`; SGLang advances
  twice, and the kernel keeps the iteration's count.
- **A default schedule, an `admit` method, `admit from q`, `uses …
  overlap`, a `batch` block, `memory` from `growing`**: a rule back in the
  language, a second place for what is said, or (TGI) a pool that never grows.
- **A device pool always engine-admitted, the Lean fragment taught it**:
  `deployment()` requires an engine-admitted pool to preempt nothing, and
  `kv` preempts. Not needed: `on D` is not engine-admitted.
- **A device pool engine-admitted where a hold names it first** (#421): the
  form this document first had. `pool kv on gpu` alone did not say whether
  the IR had `admit_via`; the holds' order did, so a memory admitted as
  soon as it fits, which the kernel writes and the queueing papers assume,
  could not be written, and `kv cap inf` changed when a request entered.
  The pool now says it, `on E.D`.
- **`pool kv on gpu { admit via E; }`, or an opt-out word**: the kernel's
  option back on an engine's pool, or a word for the other case. Rejected:
  `pool reqs on E` already says by its owner who admits, and `on E.D` says
  it the same way for the device's capacity.
- **An `each at most` that computes its cap**: the caps are constants, so
  vLLM's adaptive threshold, `max(long_prefill_token_threshold,
  input_budget // num_eligible_reqs)` (`scheduler.py:617-622`, off by
  default), cannot be written as an engine's, where a `chunk` could. Kept:
  a constant is what lets a cap of 0 or below be refused before the run;
  `max(k, e)` with `k > 0` constant would keep that and is the opening when
  a program needs it.
- **A schedule `let` read in `only` (#412)**, to name the predicate
  `advance running` and `admit waiting` share: a `let` is read as the
  iteration starts, and `only` for each request with the residents as they
  stand, so `running.decoding` would mean two values by where it is
  written (a preemption in the iteration changes it). A `def` names it
  already and is read where it stands, so the IR stays `serve only (p)`.
- **The drawing in two vocabularies, or with the device's name (#413)**:
  `serq draw` labels a step stage `engine` and `tokens cap`, and a pool
  `on S` or `on S's device`, whichever form the program used. The IR keeps
  no form, so telling them apart would need the frontend's spans beside
  the IR, and a drawing from the IR alone would differ from one from the
  text. Naming the device would need it in the IR, where no value moves
  (§Lowering) and a label does not earn a field.
- **`batch.…` and `arrive batch(n)` (#416)**: `batch` names the iteration's
  batch in an engine and the sessions at time 0 in a workload. Kept: one is
  a value after a dot in an engine's clauses, the other a call in
  `workload`, so no place reads both; `iteration.…` would name the step,
  not what it computes.
- **A queue's `pool reqs on engine`, or `on gpu` falling back to a
  top-level `gpu`**: `engine` is the keyword, and the queue's name is the
  name its stage already has; a fallback would give one spelling two
  meanings by whether the queue declares `gpu`, and a pool of the member
  would be one shared by all, or another queue's.
- **Open**: names for a run's own attributes (`decoding`, `remaining`) and
  the eviction key's `waiting` (`src/frontend/link.rs:236`), which reads as
  the list; the queue a hold waits in; `exclusive
  prefill`; `granule`. The name `stage` is now `fifo`, `ps` and `delay`'s
  alone ([One engine, one spelling](one-engine-spelling.md)).
