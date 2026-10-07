# Engines on devices

A step engine is written as what it is: a scheduler that runs on a device,
holds the request slots it admits, and says each iteration what it does.
The device says what hardware there is. A pool is a capacity of one of
them, declared with the rules that hand it out. Everything here is
parse-time sugar over today's kernel; the IR does not move.

## The problem

`examples/multi-turn/vllm.sq` today:

```
let a = 2e-5;          // compute per scheduled token (s)
let omega = 2e-4;      // weights read per iteration (s)
let beta = 2e-9;       // KV read per iteration per resident context token (s)
…
  pool kv { cap blocks * bs; block bs; evict lru; preempt lifo; }
  pool reqs { cap max_seqs; admit via engine; }   // the engine's step admits the waiting, FCFS

  stage engine : step {
    budget B;
    chunk long_prefill(reqs, chunk_cap);
    cost c0 + max(omega + beta * (kv_decode + kv_prefill), tokens * a);
    memory kv;
  }
  stage tool : delay;
```

A serving engineer reading it cannot see six things the program means.

1. **The resources.** The cost is a roofline: the left arm of `max` is HBM
   traffic (the weights, then the resident KV), the right arm compute. Neither
   resource is named; the comments on three constants say it. That the arms
   overlap (`max`, where a serial engine would add) does not read as a choice.
2. **Hardware or configuration.** `pool kv` is the GPU's memory;
   `pool reqs` is `max_num_seqs`, an operator's number. `budget` and `chunk`
   are operator's numbers too. The two pools have one form, and nothing
   separates what the device can do from what the scheduler was told.
3. **Time or policy.** `step { … }` holds the rule for how long an iteration
   takes (`cost`) beside the rules for which requests it serves (`budget`,
   `chunk`, `serve`, `iteration`, `state`), in one block (#395). `fifo(c)`
   and `ps(φ)` say only the first. The linker branches on `Step` in six places
   (`src/ir.rs`), each a rule that holds of one kind of stage and not of the
   others: `prefill`/`decode`, `growing`, `state`, an iteration's claims,
   `budget_left`, `admit via`.
4. **The engine's KV.** `memory kv` says which pool the engine reads as its
   residents' context. Every request already holds `kv` in its `hold`; the
   line repeats which of the pools it holds is the one the engine attends
   over.
5. **The engine's queues.** `admit via engine` sits on the pool, so which
   queues the engine admits, and in what order, is read off the pools'
   declaration order, not the engine.
6. **The iteration.** With no `iteration { … }` body, the engine runs vLLM's
   `schedule()` (serve the residents, then admit while the iteration has not
   preempted), a procedure the language supplies (criterion 2;
   [engine neutrality](engine-neutrality.md)).

## The design

### `device`

```
device NAME [ '[' N ']' ] {
  RESOURCE ( PARAM ) = expr ;      -- a time resource: the time a demand takes on it
  RESOURCE cap expr ;              -- a capacity: what it holds
}
```

A time resource is a function from a demand to time, inlined where it is
called, as a `def` is. It is read only in an engine's `time`. A capacity is
declared as a pool with `pool RESOURCE on NAME`. One that is not is a link
error, since nothing can hold it. The two kinds differ by form, a parameter
or a `cap`, so no name is read as either (criterion 0).

The time resources are functions, not rates. The programs' constants come
from a fit (`a`, `omega`, `beta` on the A100 testbed) and are already times.
A rate in bytes per second would read better and would need numbers the fit
does not have: the bytes of the weights and of a token's KV.

### `engine`

```
engine NAME [ '[' N ']' ] on DEVICE {
  RESOURCE cap expr ;              -- a capacity the engine holds: request slots
  budget expr ;                    -- tokens per iteration
  chunk expr ;  granule expr ;     -- per-request tokens per iteration, as today
  time expr ;                      -- how long an iteration takes, over the device's time resources
  state NAME = expr ;              -- as today
  iterate { istmt* }               -- what each iteration does
}
```

- **`time`** is today's `cost`, renamed: a request's `cost(engine, n)` is
  its work, an engine's `cost` was the iteration's length, and one word meant
  both (criterion 0). Calling the device's resources by name writes the
  roofline as one:

  ```
  time c0 + max(hbm(kv_decode + kv_prefill), compute(tokens));
  ```

  An engine whose transfers and compute do not overlap writes `+`. The
  opposite needs no keyword.
- **A capacity in an engine** is the scheduler's: `reqs cap max_seqs` is
  `max_num_seqs`. It becomes a pool as the device's does,
  `pool reqs on engine`. `budget` stays a clause, not a pool. A request does
  not hold it; the engine refills it every iteration. Something a request
  holds is a pool, and something an iteration renews is the engine's.
- **`iterate`** is the iteration, required. vLLM's procedure is written out:
  `iterate { serve; admit while (!preempted); }`. `serve only (p)`,
  `serve by (…)`, `serve decode first` and `serve exclusive prefill` move
  from stage clauses into the body as its `serve` statement. That leaves one
  place to write an iteration rather than three: a stage clause, a body, or
  nothing.
- **No `memory`.** The engine's KV is the pool on its device.

### `pool … on`

```
pool NAME on DEVICE { poolopt* }   -- DEVICE's capacity NAME
pool NAME on ENGINE { poolopt* }   -- ENGINE's capacity NAME
```

A pool takes its name and its `cap` from the capacity it is. The body is
the rules that hand the capacity out: `block`, `evict`, `preempt`, `queue`,
`spill`. A pool on an engine, or on the device the engine runs on, is
admitted by that engine, so `admit via` is not written. Its queues are tried
in the order the pools are declared, as today.

A pool on no device or engine (`pool live { cap 1; }` in
`examples/multi-turn/replica.sq`, a whole-session limit) is a pool as
today: `cap` written, admitted at settle time.

A family follows its device: under `device gpu[N]`, `pool kv on gpu` is
`kv[N]`, and `engine E[N] on gpu` reads `kv[i]` from `E[i]`. The pool's
size is not written twice, so it cannot disagree.

### Link errors

Each says why in its message.

- `pool X on D`, where D has no capacity `X`, or `X` is a time resource.
- A pool `on` something that writes `cap` or `admit via`. Its capacity and
  its admitter are already said.
- A capacity that no pool declares.
- Two pools on one engine's device. Which of them is the engine's KV would
  be a choice the program did not make. An engine with a second memory
  (an encoder cache beside the KV) gets a clause naming its KV when one is
  written.
- Two engines on one device. Who admits its pools would be ambiguous.
- A time resource read outside `time`.

## Lowering

| Written | Kernel |
|---|---|
| `device D { X cap M; }` + `pool X on D { o }` | `pool X { cap M; o; admit via E; }`, E the engine on D |
| `engine E on D { R cap S; … }` + `pool R on E { o }` | `pool R { cap S; o; admit via E; }` |
| `engine E on D { time T; budget B; …; iterate { b } }` | `stage E : step { cost T; budget B; …; memory X; iteration { b } }`, X the pool on D |
| `iterate { serve; admit while (!preempted); }` | no `iteration` field (vLLM's procedure) |
| `iterate { serve only (p); admit only (p) while (!preempted); }` | `serve only (p)` |
| a device's time resource `f(k)` | its body, inlined |

The last three rows are what keep the IR where it is. The interpreter, the
Lean generator (`scripts/gen_lean_oracle.py`'s `only_body`) and the oracle
read a vLLM engine as a step with no body. A frontend that wrote the body
out would give that program another IR: `iteration { serve; admit while
(!preempted); }` added to `examples/multi-turn/vllm.sq` today links, and its
IR differs from the program's. It would also drop out of the Lean fragment,
which accepts no body but `serve only`'s. So the parser writes the canonical
form, as the linker already does for `serve only (p)`.

Two parts were checked against today's programs:

- **Naming the resources.** `def compute(t) { t * a }` and
  `def hbm(k) { omega + beta * k }` with
  `cost c0 + max(hbm(kv_decode + kv_prefill), compute(tokens))` in
  `examples/multi-turn/vllm.sq` gives byte-identical `serq ir` output.
- **A device's pool admitted by its engine.** Of the 16 programs with a step
  engine's `memory`, the 4 whose holds name `kv` first (`tgi`,
  `llmd_nixl_pull`, `pd_batching`, `vllm_nixl_push`) already mark it
  `admit via`. In the other 12 no hold waits in `kv`'s queue, so the
  `admit via` the lowering adds is never exercised. This is a scan of the
  programs' text, not a proof. The implementation's IR comparison per
  program is the check.

## Before and After

The After does not parse yet. Its kernel is the Before.

`examples/multi-turn/vllm.sq`:

```
  device gpu {
    compute (t) = t * a;                   // time for t scheduled tokens
    hbm (k)     = omega + beta * k;        // time to read the weights and k KV tokens
    kv cap blocks * bs;                    // num_gpu_blocks × block_size
  }
  engine engine on gpu {
    reqs cap max_seqs;                     // max_num_seqs
    budget B;                              // max_num_batched_tokens
    chunk long_prefill(reqs, chunk_cap);   // long_prefill_token_threshold
    time c0 + max(hbm(kv_decode + kv_prefill), compute(tokens));
    iterate { serve; admit while (!preempted); }   // vLLM's schedule()
  }
  pool kv on gpu { block bs; evict lru; preempt lifo; }
  pool reqs on engine { queue fifo; }      // the waiting queue
  stage tool : delay;
```

`examples/engines/sglang.sq`: the body is the program's already, and it
moves as it is.

```
  engine engine on gpu {
    reqs cap max_run;
    budget B;
    time max(hbm(kv_decode + kv_prefill), compute(tokens));
    state ratio = r0;
    state backlog = 0;    // requests were waiting when the last iteration ended
    iterate { … as today … }
  }
```

`examples/engines/tgi.sq`. TGI reserves the whole sequence at admission and
never grows, which is why `memory` cannot be inferred from `growing`.
Placement says it:

```
  device gpu { compute (t) = t * a; hbm (k) = omega + beta * k; kv cap T; }
  pool kv on gpu { block 1; evict lru; preempt none; }
  engine engine on gpu {
    budget B;
    time max(hbm(kv_decode + kv_prefill), compute(tokens));
    state just = 0;                // the last forward admitted
    iterate { serve; branch (just == 0 || residents == 0) { admit; } set just = admitted > 0; }
  }
```

The decode pod of `examples/pd-disaggregation/llmd_nixl_pull.sq`, inside a
`queue`. Its engine is named after the queue, as its stage is today:

```
  queue D[ND] : decode {
    device gpu { compute (t) = t * a; hbm (k) = omega + beta * k; kv cap blocksD * bs; }
    engine on gpu {
      reqs cap max_seqsD;
      budget B;
      time c0 + max(hbm(kv_decode + kv_prefill), compute(tokens));
      iterate { serve; admit while (!preempted); }
    }
    pool reqs on D { queue fifo; }                                    // skipped_waiting: the KV arrived
    pool kv on gpu { block bs; evict lru; preempt lifo; queue fifo; }  // waiting: new requests
    nic ps(BwD);
    …
  }
```

## What it earns

- **Criterion 1.** The device lists what the hardware has. The engine lists
  what the operator set and what an iteration does. A pool line says whose
  capacity it is. A vLLM reader finds `max_num_seqs`, `max_num_batched_tokens`
  and `long_prefill_token_threshold` together in the engine, and the KV cache
  on the GPU.
- **Criterion 2.** The iteration has no default. vLLM's procedure is a body
  the program writes, as SGLang's and TGI's are. The roofline's `max` is the
  program's, as is its opposite.
- **Criterion 0.** `cost` means one thing. A pool's capacity is written in
  one place. `memory` is not a second statement of what placement says.
- **Criterion 3.** The six step-only checks become checks of form. A
  `prefill` on a `stage` or a `fifo` on an engine is not the same kind of
  declaration, and the error says so before any rule is consulted.

## The price

Parser and linker work, and the documentation:

- the frontend: `device`, `engine`, `pool … on`, `iterate`, the lowering
  above, and the link errors;
- every program with a step stage (21) is rewritten. The old spellings are
  then refused with the new one in the message, as
  [one admission](one-admission.md) retired its: two spellings of one
  engine would be two constructs with one meaning;
- `docs/language.md` §Stages and §Pools, `docs/api/stage.md`,
  `docs/api/pool.md`, the tutorials;
- `serq draw`, which reads the IR and does not move, and `serq target`,
  which reads `memory` from the IR and does not move.

Built as a stack, each part with its programs' IR compared before and after:
`device` and `pool … on DEVICE`; `engine … on` with `pool … on ENGINE`;
`iterate`.

## Self-critique

- **Split the IR (#395).** `CStageKind::Step` into a time rule and a
  separate schedule node would make criterion 3 a type, not a form. Not now:
  the two halves are used one to one, so the split moves the interpreter, the
  Lean generator, the oracle IR and `serving-queue-theory` to rename a node.
  It earns a version when something uses the halves apart. One candidate is
  a schedule shared by several engines.
- **An engine that hands its batch to a station.** An iteration is a job of
  work `time(batch)` on a `fifo(1)`, and vLLM is built that way.
  `EngineCore` schedules a batch, hands it to the executor and queues it
  (`engine/core.py:214-220, 673-729`), up to `max_concurrent_batches`. That
  is the pipeline depth, or 2 under asynchronous scheduling
  (`config/vllm.py:592-602`). Writing the engine as a scheduler over a
  `fifo(c)` would let a program say how many batches are in flight, which
  serQ fixes at one today. It needs a job that a scheduler creates and a
  rule for a request in two batches at once. Pipeline parallelism is a
  tandem of stations, not a `fifo(c)`. It is its own design.
- **`uses compute (…), hbm (…) overlap`.** Demands per resource, with a
  keyword for how they combine. Rejected: named arms in `max` say the same,
  and the opposite is `+`, not a second keyword.
- **A `batch` block in the engine.** Everything in an engine is per
  iteration, so `batch` adds no information.
- **`memory` from `growing`.** The pool a run grows is not always its
  context. TGI reserves up front and grows nothing (`examples/engines/tgi.sq`)
  and still has `memory kv`.
- **A default `iterate`.** Rejected for criterion 2. Brevity belongs in a
  library: `iterate { vllm_schedule(); }` from `lib/vllm.sq`. A `def` cannot
  hold iteration statements today (`iteration { tgi_forward(); }` is refused:
  "an iteration takes `serve`, `admit`, `branch` and `set`"), and widening it
  is sugar. A library is not the place for the request side's `hold`, which
  #399 moved back into the programs. Unlike the hold, a scheduler's
  procedure is the same text in every program that runs that scheduler.
- **`admit from q` in the engine.** It would state the engine's queues on the
  engine, in order, but beside `pool reqs on engine` it says the queue twice.
  The order of declaration stays the order of admission.
- **What is not decided here.**
  - **The queue a hold waits in.** A hold waits in the queue of the pool it
    names first, so in the decode pod, vLLM's `skipped_waiting` and
    `waiting` are two pools' queues chosen by the order of a hold's pools.
    That is a rule of `hold`, not of the engine, and a separate issue.
  - **`serve exclusive prefill`.** It remains a statement whose meaning is
    the language's. A body cannot take back decodes already chosen
    ([exclusive prefill](exclusive-prefill.md)).
  - **The name `stage`.** Without engines, `stage` covers `fifo`, `ps` and
    `delay` only. Whether it should be renamed is its own change; it touches
    every program.
