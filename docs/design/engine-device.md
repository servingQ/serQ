# Engines on devices

A step engine is written as what it is: a scheduler on a device. It holds
the request slots it admits, schedules a batch each iteration, and executes
it. The device says what hardware there is. A pool is a capacity of one of
them, declared with the rules that hand it out. A value the schedule reads
is named after the list it describes. Everything here is parse-time sugar:
no IR node, field or version is added.

## The problem

`examples/multi-turn/vllm.sq` today, with `lib/vllm.sq`'s `long_prefill`:

```
let a = 2e-5;          // compute per scheduled token (s)
let omega = 2e-4;      // weights read per iteration (s)
let beta = 2e-9;       // KV read per iteration per resident context token (s)
…
def long_prefill(reqs, c) { residents + queued(reqs) > 1 ? c : 0 }
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

A serving engineer reading it cannot see eight things the program means.

1. **The resources.** The cost is a roofline: the left arm of `max` is HBM
   traffic (the weights, then the resident KV), the right arm is compute.
   Neither resource is named; the comments on three constants say it. That
   the arms overlap (`max`, where a serial engine would add) does not read
   as a choice.
2. **Hardware or configuration.** `pool kv` is the GPU's memory;
   `pool reqs` is `max_num_seqs`, an operator's number. `budget` and `chunk`
   are operator's numbers too. The two pools have one form, and nothing
   separates what the device can do from what the scheduler was told.
3. **Choosing or executing.** `step { … }` holds the rule for how long an
   iteration takes (`cost`) beside the rules for which requests it serves
   (`budget`, `chunk`, `serve`, `iteration`, `state`), in one block (#395).
   `fifo(c)` and `ps(φ)` say only the first. The linker branches on `Step`
   in six places (`src/ir.rs`), each a rule that holds of one kind of stage
   and not of the others: `prefill`/`decode`, `growing`, `state`, an
   iteration's claims, `budget_left`, `admit via`.
4. **What `budget` and `chunk` limit.** Both limit the tokens an iteration
   computes: `budget` all of them, `chunk` one request's. vLLM applies its
   threshold to every request (`scheduler.py:675-676`); a decode has one
   token left and is never cut. Nothing in the two words says they limit one
   quantity, and that quantity is `tokens`, which the cost reads.
5. **Logic in a declaration.** `long_prefill`'s condition is decided every
   iteration from the engine's lists, and it sits in the engine's
   declaration, behind a library name. `chunk 0` means no cap and reads as
   a cap of zero.
6. **Values with no origin.** A schedule reads `residents`, `queued(reqs)`,
   `preempted`, `admitted` and `tokens`, which no line of the program
   defines. What each counts, and over which iteration, is in the
   documentation. `queued(reqs)` also counts one queue of the engine's
   two: vLLM counts `running`, `waiting` and `skipped_waiting`
   (`scheduler.py:609-611`), which `lib/vllm.sq`'s comment admits it leaves
   out.
7. **The engine's KV and queues.** `memory kv` says which pool the engine
   reads as its residents' context. Every request already holds `kv` in its
   `hold`. `admit via engine` sits on the pool, so which queues the engine
   admits, and in what order, is read off the pools.
8. **The iteration.** With no `iteration { … }` body, the engine runs vLLM's
   `schedule()` (serve the residents, then admit while the iteration has
   not preempted), a procedure the language supplies (criterion 2;
   [engine neutrality](engine-neutrality.md)).

## The design

The shape follows vLLM's own engine step, which schedules a batch, has the
executor run it, and updates from its output (`engine/core.py:673-729`).

### `device`

```
device NAME [ '[' N ']' ] {
  RESOURCE ( PARAM ) = expr ;      -- a time resource: the time a demand takes on it
  RESOURCE cap expr ;              -- a capacity: what it holds
}
```

A time resource is a function from a demand to time, inlined where it is
called, as a `def` is. It is read only in an engine's `execute`. A capacity
is declared as a pool with `pool RESOURCE on NAME`. One that is not is a
link error, since nothing can hold it. The two kinds differ by form, a
parameter or a `cap`, so no name is read as either (criterion 0).

The time resources are functions, not rates. The programs' constants come
from a fit (`a`, `omega`, `beta` on the A100 testbed) and are already times.
A rate in bytes per second would read better and would need numbers the fit
does not have: the bytes of the weights and of a token's KV.

### `engine`

```
engine NAME [ '[' N ']' ] on DEVICE {
  RESOURCE cap expr ;              -- a capacity the engine holds: request slots
  tokens cap expr ;                -- the tokens one iteration computes
  granule expr ;                   -- as today
  state NAME = expr ;              -- as today
  schedule { sstmt* }              -- chooses the batch
  execute ( expr ) ;               -- how long the batch takes
}
```

An engine holds what lasts across iterations and what bounds each one.

- **`reqs cap max_seqs`** is a capacity the engine holds, `max_num_seqs`.
  It becomes a pool as the device's capacities do, `pool reqs on engine`,
  and a request holds a slot from admission to its end.
- **`tokens cap B`** bounds `tokens`, the tokens the iteration computes:
  vLLM's `token_budget`, `max_num_batched_tokens` (`scheduler.py:577`).
  `tokens` starts at 0 each iteration, so its cap is an iteration's, and
  no request holds it. `tokens` is the language's name, so it cannot be a
  pool (`pool tokens` is a link error), and one name means one quantity. It
  is the quantity `execute` reads and `budget_left(engine)` is `B - tokens`.
  The line is required; an engine with no budget writes `tokens cap inf`.
- **`schedule { … }`** chooses the batch. Its body is required and runs
  once per iteration, in order:

  ```
  let NAME = expr ;                                         -- before every other statement
  advance running [only ( p )] [ORDER] [each at most ( e )] ;     -- today's serve
  admit waiting [only ( p )] [while ( e )] [each at most ( e )] ;  -- today's admit
  exclusive prefill [each at most ( e )] ;                        -- today's serve exclusive prefill
  branch ( e ) { … } [else { … }]
  set NAME = e ;
  ```

  `advance running` gives the runs already on the engine their tokens for
  the iteration: vLLM's running loop (`scheduler.py:626-823`). `admit
  waiting` takes the heads of the engine's queues and gives each its tokens
  as it is admitted: vLLM's waiting loop (`:872-1128`). Both spend the one
  `tokens` cap, in the order the body writes them. `each at most (e)` is the
  cap on one run's tokens, read for each, as `only (p)` is: vLLM's cap on
  `num_new_tokens`, which it applies to the running (`:675-676`) and to the
  waiting (`:1115-1128`). Not writing it is no cap, and `inf` is too; there
  is no 0 that means none. A decode is not named. It has one token left,
  and the cap never reaches it.

  The clauses follow their statement without a comma, as `only` and `by`
  do today, in one order: `only`, then `ORDER` or `while`, then `each at
  most`, so a statement has one spelling. `each` says the cap is one run's;
  `at most (e)` alone would also read as the statement's total.

  `exclusive prefill` is neither list's: a waiting prefill that fits
  displaces the running decodes already chosen. It stays a rule whose
  meaning is the language's ([exclusive prefill](exclusive-prefill.md)).

  vLLM's procedure is written out:

  ```
  schedule {
    let cap = running.count + waiting.count > 1 ? max_per_request : inf;
    advance running each at most (cap);
    branch (running.preempted == 0) {
      admit waiting each at most (cap);
    }
  }
  ```

  vLLM skips its waiting loop in a step that preempted
  (`scheduler.py:869`, `if not preempted_reqs`). Its waiting loop does not
  preempt, so this is a branch, not a loop condition.
- **`execute (T)`** is how long the scheduled batch takes. `T` reads the
  batch (`tokens`, `kv_decode`, `kv_prefill`, …) once it is chosen
  (`Moment::Step`), and calls the device's resources by name, which writes
  the roofline as one:

  ```
  execute (c0 + max(hbm(kv_decode + kv_prefill), compute(tokens)));
  ```

  An engine whose transfers and compute do not overlap writes `+`. The
  opposite needs no keyword. `cost` meant both a request's work
  (`cost(engine, n)`) and an iteration's length; the second is now
  `execute`, so `cost` means one thing (criterion 0).
- **No `memory`.** The engine's KV is the pool on its device.

Admission is not a method of its own. When an engine admits is the place of
`admit waiting` in the body, and that place is where engines differ. SGLang
admits between two `advance running`s, and TGI admits only when the last
forward did not. A method the engine called by itself could not say either.
Whom it admits, in what order and how many are already said elsewhere: the
pool's `queue`, the engine's `cap`, and the hold's `at admission` header.

### What a schedule reads

Every value a schedule reads is named after what it describes:

| Written | Is | Today |
|---|---|---|
| `running.count` | the runs on the engine | `residents` |
| `running.decoding` | of them, those decoding | `decoders` |
| `running.preempted` | the runs preempted out of `running` so far this iteration | `preempted` |
| `waiting.count` | the holds waiting in the queues the engine admits | `queued(p)`, summed over them |
| `waiting.admitted` | the holds admitted so far this iteration | `admitted` |
| `tokens` | the tokens scheduled so far this iteration (in `execute`, the batch's) | `tokens` |

`running` and `waiting` are vLLM's two lists (`self.running`,
`self.waiting`); a vLLM reader knows what they hold. `waiting.count` is
every queue the engine admits, as vLLM counts `waiting` and
`skipped_waiting` together. `queued(p)` stays for a program that wants one
queue.

The attributes `only (p)`, `ORDER` and `each at most (e)` read for each run
(`decoding`, `remaining`, `admission`) are the run's own. They are named in
the open questions below.

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
- A time resource read outside `execute`.
- An engine without `tokens cap`, or a `tokens cap` or `each at most` whose
  outcome is a constant at or below 0. The kernel reads a 0 cap per run as
  no cap, so the program would mean one thing and run another. A stricter
  check does not bump `IR_VERSION`. An outcome that reads state cannot be
  checked before the run.
- A `let` after another statement, or read anywhere but `each at most`. A
  `let` is read as the iteration starts, where the kernel reads its
  per-run cap. Read later, its value would be that moment's, which the
  kernel has no place to keep.
- Two `each at most` in one body that differ, or one written on some
  statements and not others. The kernel has one per-run cap for the
  iteration.

## Lowering

| Written | Kernel |
|---|---|
| `device D { X cap M; }` + `pool X on D { o }` | `pool X { cap M; o; admit via E; }`, E the engine on D |
| `engine E on D { R cap S; … }` + `pool R on E { o }` | `pool R { cap S; o; admit via E; }` |
| `tokens cap B` | `budget B` |
| `each at most (c)`, with its `let`s substituted | `chunk c'`: `c` with an `inf` outcome (the whole of it, or a branch of a `?:`) written `0`; `0` when no statement has one |
| `advance running` / `admit waiting` | `CIter::Serve` / `CIter::Admit` |
| a body that is `[let …] advance running [ORDER] …; branch (running.preempted == 0) { admit waiting …; }` | no `iteration`, `serve ORDER` (vLLM's procedure) |
| the same with `exclusive prefill` for `advance running` | no `iteration`, `serve exclusive prefill` |
| `running.count`, `running.decoding`, `running.preempted`, `waiting.admitted` | `residents`, `decoders`, `preempted`, `admitted` |
| `waiting.count` | `queued(p₁) + … + queued(pₙ)`, the pools E admits |
| `execute (T)` | `cost T` |
| the engine | `stage E : step { …; memory X; }`, X the pool on D |
| a device's time resource `f(k)` | its body, inlined |

The vLLM row keeps that program's iteration where the kernel has it. The
interpreter, the Lean generator (`scripts/gen_lean_oracle.py`'s `only_body`
and `chunk_rule`), `scripts/lean_drt.py`, `scripts/lean_bench.py` and the
oracle read a vLLM engine as a step with no body. A frontend that wrote the
body out would give that program another IR: today's
`iteration { serve; admit while (!preempted); }` added to
`examples/multi-turn/vllm.sq` links, but its IR differs from the program's,
and the Lean fragment accepts no body but `serve only`'s. So the parser
writes the canonical form, as the linker already writes `serve only (p)` as
a body. The vLLM body's `branch (running.preempted == 0)` and the kernel's
`admit while (!preempted)` are one iteration because admission does not
preempt.

Some programs' IR still moves, with no new node or field and the same
meaning:

- **A device's pool gains `admit_via`.** Of the 16 programs with a step
  engine's `memory`, the 4 whose holds name `kv` first (`tgi`,
  `llmd_nixl_pull`, `pd_batching`, `vllm_nixl_push`) already mark it
  `admit via`. In the other 12 no hold waits in `kv`'s queue, so the field
  the lowering adds is never exercised. This is a scan of the programs'
  text, not a proof.
- **`waiting.count` is a sum.** vLLM's per-run cap reads `residents +
  queued(reqs)` today; written with `waiting.count` it reads `residents +
  queued(reqs) + queued(kv)`. The `kv` queue is empty in the single-engine
  programs, so the cap is the same, and in the decode pod the sum is
  vLLM's count.

The implementation compares each program's runs before and after, and
regenerates the oracle IR (`make oracle-ir`) and the Lean theorems that
move with it.

One part was checked against today's programs: `def compute(t) { t * a }`
and `def hbm(k) { omega + beta * k }` with
`cost c0 + max(hbm(kv_decode + kv_prefill), compute(tokens))` in
`examples/multi-turn/vllm.sq` give byte-identical `serq ir` output.

## Before and After

The After does not parse yet. Its kernel is the Before, but for the two
moves above.

`examples/multi-turn/vllm.sq`:

```
  device gpu {
    compute (t) = t * a;                   // time for t scheduled tokens
    hbm (k)     = omega + beta * k;        // time to read the weights and k KV tokens
    kv cap blocks * bs;                    // num_gpu_blocks × block_size
  }
  engine engine on gpu {
    reqs cap max_seqs;                     // max_num_seqs
    tokens cap B;                          // max_num_batched_tokens
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
  pool reqs on engine { queue fifo; }      // the waiting queue
  stage tool : delay;
```

`max_per_request` is the program's name for `long_prefill_token_threshold`,
and `lib/vllm.sq` keeps only `reusable`.

`examples/engines/sglang.sq`, whose body is the program's already:

```
  engine engine on gpu {
    reqs cap max_run;
    tokens cap B;
    state ratio = r0;
    state backlog = 0;    // requests were waiting when the last iteration ended
    schedule {
      // fully idle since the last iteration: no batch, and none waited
      branch (running.count == 0 && backlog == 0) { set ratio = r0; }
      advance running only (!decoding);                  // the chunked request
      admit waiting;                                     // and new prefills, alone
      branch (tokens == 0) {                             // no prefill: a decode batch
        advance running;
        branch (running.preempted > 0) {
          set ratio = max(r_min, min(1, retract_steps / M));
        } else {
          set ratio = max(r_min, ratio - r_decay);
        }
      }
      set backlog = waiting.count > 0;
    }
    execute (max(hbm(kv_decode + kv_prefill), compute(tokens)));
  }
```

`examples/engines/tgi.sq`. TGI reserves the whole sequence at admission and
never grows, which is why `memory` cannot be inferred from `growing`.
Placement says it:

```
  device gpu { compute (t) = t * a; hbm (k) = omega + beta * k; kv cap T; }
  pool kv on gpu { block 1; evict lru; preempt none; }
  engine engine on gpu {
    tokens cap B;
    state just = 0;                // the last forward admitted
    schedule {
      advance running;
      branch (just == 0 || running.count == 0) { admit waiting; }
      set just = waiting.admitted > 0;
    }
    execute (max(hbm(kv_decode + kv_prefill), compute(tokens)));
  }
```

The decode pod of `examples/pd-disaggregation/llmd_nixl_pull.sq`, inside a
`queue`. Its engine is named after the queue, as its stage is today:

```
  queue D[ND] : decode {
    device gpu { compute (t) = t * a; hbm (k) = omega + beta * k; kv cap blocksD * bs; }
    engine on gpu {
      reqs cap max_seqsD;
      tokens cap B;
      schedule {
        advance running;
        branch (running.preempted == 0) { admit waiting; }
      }
      execute (c0 + max(hbm(kv_decode + kv_prefill), compute(tokens)));
    }
    pool reqs on D { queue fifo; }                                    // skipped_waiting: the KV arrived
    pool kv on gpu { block bs; evict lru; preempt lifo; queue fifo; }  // waiting: new requests
    nic ps(BwD);
    …
  }
```

## What it earns

- **Criterion 1.** The device lists what the hardware has. The engine lists
  what the operator set (`max_num_seqs`, `max_num_batched_tokens`), how it
  schedules and how long execution takes. A pool line says whose capacity
  it is. The schedule reads like vLLM's `schedule()`: the threshold for this
  step, the running loop, the waiting loop unless the step preempted.
- **Criterion 2.** The schedule has no default. vLLM's procedure is a body
  the program writes, as SGLang's and TGI's are, and the order of
  `advance running` and `admit waiting` is the program's. The roofline's
  `max` is the program's, as is its opposite. The condition of vLLM's
  per-run cap is the program's text.
- **Criterion 0.** `cost` means one thing. `tokens` is one quantity, capped
  where it is declared and read where it is spent. No cap is `inf`, not 0.
  A pool's capacity is written in one place, and `memory` is not a second
  statement of what placement says.
- **Criterion 3.** The six step-only checks become checks of form. A
  `prefill` on a `stage` or a `fifo` on an engine is not the same kind of
  declaration, and the error says so before any rule is consulted.

## The price

Parser and linker work, and the documentation:

- the frontend: `device`, `engine`, `pool … on`, `schedule`, `execute`, the
  values of the table above, the lowering, and the link errors;
- every program with a step stage (21) is rewritten. The old spellings
  (`stage … : step`, `memory`, `admit via`, `budget`, `chunk`, `cost` on a
  step, `iteration`, `serve` and `admit` as statements, and the bare
  `residents`, `decoders`, `preempted` and `admitted`) are then refused
  with the new one in the message, as [one admission](one-admission.md)
  retired its: two spellings of one engine would be two constructs with one
  meaning;
- the oracle IR files and the Lean theorems move with `waiting.count` and
  the device pool's `admit_via`. `scripts/lean_bench.py`'s `chunk_of` takes
  vLLM's rule with one `queued(p)` and learns the sum, and
  `scripts/gen_lean_oracle.py` must accept a device pool's `admit_via`.
  Whether the Lean fragment takes both is the first thing the
  implementation checks;
- `lib/vllm.sq` loses `long_prefill`, and `examples/oracle/vllm_request.sq`
  and its scenarios say no cap as `inf` where they pass `chunk=0`;
- `docs/language.md` §Stages and §Pools, `docs/api/stage.md`,
  `docs/api/pool.md`, the tutorials;
- `serq draw` and `serq target` read the IR and do not move.

Built as a stack, each part with its programs' runs compared before and
after: `device` and `pool … on DEVICE`; `engine … on` with `pool … on
ENGINE`; `schedule` and `execute`.

## Self-critique

- **Split the IR (#395).** `CStageKind::Step` into a time rule and a
  separate schedule node would make criterion 3 a type, not a form. Not now:
  the two halves are used one to one, so the split moves the interpreter, the
  Lean generator, the oracle IR and `serving-queue-theory` to rename a node.
  It earns a version when something uses the halves apart. One candidate is
  a schedule shared by several engines.
- **An engine that hands its batch to a station.** An iteration is a job of
  work `T` on a `fifo(1)`, and vLLM is built that way. `EngineCore`
  schedules a batch, hands it to the executor and queues it
  (`engine/core.py:214-220, 673-729`), up to `max_concurrent_batches`. That
  is the pipeline depth, or 2 under asynchronous scheduling
  (`config/vllm.py:592-602`). Letting `execute` run up to `c` batches at
  once would let a program say how many are in flight, which serQ fixes at
  one today. It needs a rule for a request in two batches at once, and
  pipeline parallelism is a tandem of stations, not a `fifo(c)`. It is its
  own design. `schedule` and `execute` being apart is where it would go.
- **A `step { schedule …; execute …; }` around both, or `schedule` inside
  `execute`.** vLLM's `EngineCore.step()` calls `schedule()` and then
  `execute_model()`, so the outer block is `step`, not `execute`, which is
  only the model's run. Today the kernel has one arrangement, choose and
  then execute, so the block would hold the same two lines in the same
  order in every program. It earns its place with the batch queue above,
  where the next batch is scheduled while the last executes, and that
  design adds it.
- **One `iterate` body with the budget first and the cost last.** It reads
  in order, but needs position rules to say what `schedule` and `execute`
  say by their form.
- **A `schedule budget B, per request (c)` header.** It put the iteration's
  limits apart from the engine's, and named neither the quantity they limit
  nor the thing `per request` counts: `request` is a retired keyword
  (`parser.rs` refuses it for `turn`), and the cap is on a run, not a
  request. `tokens cap B` names the quantity, and `each at most` on a list
  statement is read for each item, as `only` is.
- **The per-run cap on the engine, `tokens cap (c) per run`.** It names the
  item, but leaves the condition, which is decided every iteration from the
  lists, in a declaration. In the schedule, as a `let`, the logic sits where
  it is decided.
- **`advance running, each at most (cap)`.** The comma came from the
  English sentence. Every other clause of a statement follows it without
  one (`serve only (p) by (k)`, `admit only (p) while (e)`), so the comma made
  one clause look like another kind of construct.
- **`reqs.items` for the items capped.** `reqs` is a pool of slots, and the
  cap is on the engine's runs, which TGI has without a `reqs` pool
  (`examples/engines/tgi.sq` declares `kv` alone).
- **`decode at most (1)` beside `prefill at most (c)`.** It named the
  decode's one token as if it were a policy. vLLM has no decode rule. Every
  request gets what it has left to compute, capped (`scheduler.py:670-678`),
  and a decode has one token left. A per-mode cap would also have needed an
  IR field that only ever holds 1.
- **`chunk`, `prefill at most`, `execute at most`.** `chunk` names the
  result rather than the limit. `prefill at most` names one mode where
  vLLM's cap is every request's. `execute at most` reads as an action and
  does not separate one run's limit from the iteration's.
- **`serve` and `admit`, bare `running` and `waiting`, `start running`.**
  `serve` named the whole of serving in common speech, and in a `queue` it is
  the station's service (`serve step { … }`). Bare `running;` is a noun
  where a statement does something. `start running` says the running are
  started; they were, and the statement moves them on by their tokens.
  `advance running` and `admit waiting` are a verb and vLLM's list each.
- **`admit waiting; start waiting;`.** Admission and the admitted run's first
  tokens are one act: vLLM decides a waiting request's `num_new_tokens` as
  it admits it (`scheduler.py:1078-1128`), and a hold's header reads
  `budget_left(engine)` for the size it enters with. Apart, a program could
  admit without starting, and a request would hold a slot and KV doing
  nothing, a state vLLM does not have and the kernel cannot write.
- **The budget and the per-run cap on `admit waiting` alone.** The running
  spend the budget first and the waiting what is left, which suggested the
  budget is admission's. It is both lists': vLLM's running loop is bounded by
  it (`scheduler.py:626`, `:821`), and the per-run cap applies to both. Nor
  is "the running first, then the waiting" every engine's order (SGLang,
  TGI and `exclusive prefill` differ; V1 of
  [engine neutrality](engine-neutrality.md)), so the order is the body's.
- **A value from the statement that made it,
  `let preempted = advance running;`.** It shows where a value comes from,
  but a statement's own count and the iteration's so far differ when an
  earlier statement preempted. SGLang's body has two `advance running`s, and
  the kernel keeps only the iteration's count. `running.preempted` is that
  count, named after the list it left.
- **An `admit` method, optional.** When to admit is the body's (`admit
  waiting`'s place), and the rest is the pool's and the hold's. An optional
  method would also need a default, which is criterion 2's problem again.
- **`uses compute (…), hbm (…) overlap`.** Demands per resource, with a
  keyword for how they combine. Named arms in `max` say the same, and the
  opposite is `+`, not a second keyword.
- **A `batch` block in the engine.** Everything in an engine is per
  iteration, so `batch` adds no information.
- **`memory` from `growing`.** The pool a run grows is not always its
  context. TGI reserves up front and grows nothing and still has `memory kv`.
- **A default schedule.** Rejected for criterion 2. Brevity belongs in a
  library: `schedule { vllm_schedule(); }`. A `def` cannot hold iteration
  statements today (`iteration { tgi_forward(); }` is refused: "an
  iteration takes `serve`, `admit`, `branch` and `set`"), and widening it is
  sugar. A library is not the place for the request side's `hold`, which
  #399 moved back into the programs. Unlike the hold, a scheduler's
  procedure is the same text in every program that runs that scheduler.
- **`admit from q` in the engine.** It would state the engine's queues on the
  engine, in order, but beside `pool reqs on engine` it says the queue twice.
  The order of declaration stays the order of admission.
- **What is not decided here.**
  - **A run's own attributes.** `only (p)`, `ORDER` and `each at most (e)`
    read `decoding`, `remaining` and `admission` of each run, with no name
    for the run. A binder (`only (r => r.decoding)`) would say whose they
    are, in every key of the language at once.
  - **What `execute` reads.** `kv_decode`, `kv_prefill` and `tokens` are the
    batch's, and could be named so (`batch.kv_decode`).
  - **The queue a hold waits in.** A hold waits in the queue of the pool it
    names first, so in the decode pod, vLLM's `skipped_waiting` and
    `waiting` are two pools' queues chosen by the order of a hold's pools.
    That is a rule of `hold`, not of the engine, and a separate issue.
  - **`exclusive prefill`.** It remains a statement whose meaning is the
    language's. A body cannot take back decodes already chosen.
  - **`granule`.** It shapes one run's grant as `each at most` bounds it, and
    could be one clause with it. `granule inf`, a prefill whole or not at
    all, has no natural spelling yet, so it stays as it is.
  - **The name `stage`.** Without engines, `stage` covers `fifo`, `ps` and
    `delay` only. Whether it should be renamed is its own change; it touches
    every program.
