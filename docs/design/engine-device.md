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
  execute (c0 + max(hbm(kv_decode + kv_prefill), compute(tokens)));
}
pool kv on gpu { block bs; evict lru; preempt lifo; }
pool reqs on vllm { queue fifo; }
```

- **`device`**: time resources (demand to time, read in `execute`; functions
  because the fitted constants are times) and capacities.
- **`pool X on D`** takes its name and `cap` from D's capacity `X`; its body
  is the rules. A pool on an engine or on its device is admitted by that
  engine, and the engine's KV is the pool on its device, so `admit via` and
  `memory` go. Under `device gpu[N]`, `pool kv on gpu` is `kv[N]` and
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
  `skipped_waiting`, `scheduler.py:609-611`), `waiting.admitted`. The bare
  names are retired where these replace them (serve keys, claims, gauges).
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
engine, a queue that holds an engine named for a keyword, and a queue's
device named as the queue (`on Q` would be two things). A capacity's `cap`
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

- **`admit_via` where a queue is used.** A pool on an engine is admitted by
  it. A pool on its device is admitted by it only where a hold names the
  pool first, so waits in its queue; elsewhere that queue is empty, and the
  pool is written without `admit_via`, as the 12 programs do today (and as
  `deployment()` requires of an engine-admitted pool: no preemption).
- **`waiting.count`** is `queued(p)` summed over the pools whose `admit_via`
  is the engine, which is `queued(reqs)` where `long_prefill` read it.
- **`inf` is written 0 by the linker**, once the `let`s have their values:
  an outcome of `inf` (the whole cap, or a branch of a `?:`) is no cap,
  which the kernel writes 0 as `min(remaining, inf)` gives. The linker does
  it for every step, so an old `chunk inf` is 0 too. An engine's outcomes
  are constants once linked, so one at or below 0, which the kernel would
  read as no cap, does not link: the program never means one thing and runs
  another.

`tests/engine_device.rs` holds `vllm.sq`, `sglang.sq` and `tgi.sq` written
as engines to their programs' IR, byte for byte, as it holds each stage form
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
  `kv` preempts. Not needed: a device pool is admitted only where a hold
  waits in its queue. The cost is that `pool kv on gpu` alone does not say
  whether the IR has `admit_via`; the holds' order does, and with it which
  pool's keys may read the engine's `state` (`src/ir.rs`, "the keys of a
  pool it admits").
- **An `each at most` that computes its cap**: the caps are constants, so
  vLLM's adaptive threshold, `max(long_prefill_token_threshold,
  input_budget // num_eligible_reqs)` (`scheduler.py:617-622`, off by
  default), cannot be written as an engine's, where a `chunk` could. Kept:
  a constant is what lets a cap of 0 or below be refused before the run;
  `max(k, e)` with `k > 0` constant would keep that and is the opening when
  a program needs it.
- **A queue's `pool reqs on engine`, or `on gpu` falling back to a
  top-level `gpu`**: `engine` is the keyword, and the queue's name is the
  name its stage already has; a fallback would give one spelling two
  meanings by whether the queue declares `gpu`, and a pool of the member
  would be one shared by all, or another queue's.
- **Open**: names for a run's own attributes (`decoding`, `remaining`) and
  the eviction key's `waiting` (`src/frontend/link.rs:236`), which reads as
  the list; `batch.kv_decode`; the queue a hold waits in; `exclusive
  prefill`; `granule`; the name `stage`.
