# The scheduler and the worker

An engine is two components, as vLLM is: a scheduler that chooses what an
iteration computes, and a worker that computes it. The engine's blocks are
named for them, `scheduler { … }` and `worker { … }`, and what passes from
one to the other is the batch (#555).

This document is the record of #555's four steps. The first two are
parse-time renames: the IR does not move. Steps 3 and 4 (a request as the
engine's state, and a KV connector's load and save) extend it.

## In vLLM

`Scheduler.schedule()` returns a `SchedulerOutput`
(`vllm/v1/core/sched/output.py:232`): the tokens scheduled per request and
in total (`num_scheduled_tokens`, `total_num_scheduled_tokens`), the
requests finished and preempted, and the connector's metadata. The model
runner takes it and runs the forward; a KV connector's loads start around
the forward and its saves are waited for after it
(`kv_connector_model_runner_mixin.py:84-92`). Nothing the runner does reads
the waiting queue or a request the scheduler did not schedule.

## In serQ

```serq
  engine vllm on gpu {
    reqs cap max_seqs;
    tokens cap B;
    scheduler {
      let threshold = running.count + waiting.count > 1 ? chunk_cap : inf;
      advance running each at most (threshold);
      admit waiting while (running.preempted == 0) each at most (threshold)
        taking reqs (1), kv (min(known, reused + budget_left(vllm))) reserve (known);
    }
    worker {
      forward (c0 + max(hbm(batch.kv_decode + batch.kv_prefill), compute(batch.tokens)));
    }
  }
```

- `scheduler { … }` was `schedule { … }`; the old word is refused with the
  new one. Its statements, and what they read, are unchanged.
- `worker { forward (T); }` was `execute (T);`, refused with the new form.
  `forward` is the only item a worker takes; it is required, once. Step 4's
  copies are not the worker's (*Self-critique*).
- The worker reads the batch, `batch.…`, the device's time resources, the
  constants and the program's definitions. It does not read `running.…` or
  `waiting.…`: what the residents and the queues hold beside the batch is
  the scheduler's. `execute` read `running.count` and `waiting.count`; no
  program did, and the check is stricter, so the IR version does not move.

## Step 3: a request is the engine's state

vLLM has no procedure per request. A `Request` is state: `num_tokens` (the
prompt and what it has generated) and `num_computed_tokens` (what of them
has KV). Each iteration the scheduler gives a running request up to
`num_tokens - num_computed_tokens` tokens (`scheduler.py:670-674` for the
running, `scheduler.py:1089` for the waiting); a prefill and a decode are
that one rule with many tokens left or one. When a request's computed
tokens reach its tokens it samples one. A preemption sets
`num_computed_tokens` to 0 and keeps the generated tokens
(`scheduler.py:1561`).

An engine is that mechanism in serQ too, and its `request (…) { … }` block
goes: the programs wrote the same five lines (`kept`, `known = max(kept,
computed + 1)`, `prefill (known - c)`, `decode (output - 1 - (known -
prompt))`) to say it.

`examples/multi-turn/vllm.sq`, its comments left out:

```serq
  pool kv on gpu { block bs; evict lru; preempt lifo; }
  engine vllm on gpu {
    reqs cap max_seqs;
    tokens cap B;
    scheduler {
      let threshold = running.count + waiting.count > 1 ? chunk_cap : inf;
      advance running each at most (threshold);
      admit waiting while (running.preempted == 0) each at most (threshold)
        taking reqs (1), kv (min(request.known, request.hit + budget_left(vllm)))
          reserve (request.known)
        reuse (floor(request.known - 1, blocksize(kv)))
        cache (request.input + request.output);
    }
    worker {
      forward (c0 + max(hbm(batch.kv_decode + batch.kv_prefill), compute(batch.tokens)));
    }
  }
  …
  server {
    vllm (K + n, o);
  }
```

### The call and the state

`E (input, output);` is a request to engine `E`: `input` tokens of prompt,
`output` tokens to generate. The session waits until the request has
generated them, then goes on. The interpreter keeps, per request:

- `known`: the tokens it has, `input` and what it has generated;
- its position in the engine's KV pool (the pool `kv_decode` reads, its
  `memory`): what of `known` has KV;
- `generated`, and whether it has `finished`.

### The admission

The request waits in the queue of the first pool the admission takes. When
the scheduler's `admit waiting` reaches it, it reads, in order:

1. `request.hit`: `min(cachedin(P), reuse)`, `P` the memory pool, `reuse`
   the `reuse (e)` clause (absent: no bound);
2. `request.loaded`, what its connector loads (step 4);
3. the units and `reserve` of each pool the admission `taking`s.

The admission reads the cached prefix (at most `reuse`), as a hold with a
`cache` clause does, and the request's position starts there; `cached` is
what it read.

### The phases

- **Prefill**: the request computes `known - cached` tokens, chunked by the
  scheduler as a prefill is. At its end a request with any tokens
  (`known > 0`) samples one: `generated` and `known` grow by one.
- **Decode**: one token per iteration until `generated` is `output`, each
  sampling one.

The memory pool's allocation grows with the position (a `growing` run's
rule): a request admitted with fewer blocks than it computes takes more as
it goes, and one admitted with all of them (`ascend.sq`, `tgi.sq`) takes
none. A request with `input` 0 has no prefill and decodes from its first
iteration (`kong_svf.sq`).

### Preemption and the release

A preempted request gives its pools back, caching per `cache` what it
computed, and waits again at the head of the queue (or the tail, `requeue
tail`). It keeps `known` and `generated`; `computed` is the position it had
reached. When it has generated `output`, it is `finished`, gives its pools
back with `cache` (read then, with `finished` 1), and the session goes on.

### What the clauses read

| Name | What | Read in |
|---|---|---|
| `request.input`, `request.output` | the call's arguments | `taking`, `reuse`, `prefix`, `cache` |
| `request.known` | the tokens the request has | `taking`, `reuse`, `prefix`, `cache` |
| `request.hit` | `min(cachedin(P), reuse)` | `taking` |
| `request.finished` | 1 at the release that ends the request, 0 at a preemption | `cache` |
| `request.loaded` | what its connector loads (step 4) | `taking` |
| `request.preempted` | 1 at an admission after a preemption | `taking`, a connector's `load` |

They are read as `request.…`, so that no session attribute is named like
one.

They read the session's attributes too, as a queue's keys do
(`vllm_replay.sq`'s `hitmax`, `vllm_system_prompt.sq`'s `product`).

### What the engine records

What a program observed inside its request, the engine records, and every
record is an attribute of the session its caller reads after the call:
`E.reused` (the cached tokens its latest admission read), `E.prefill` (the
tokens its prefill computes), `E.admitted` (when it was last admitted) and
`E.first` (when its first token was sampled), so that `vllm_replay.sq`
observes `ttft = vllm.first - t0`. What each admission records, `E.reused`
and `E.prefill` (and a connector's `E.loaded`), is an observation of the
report too, under the same name: one rule for every record, so that the
caller reads any of them.

### Lean

The Lean model keeps holds and runs. `scripts/gen_lean_oracle.py`
translates a `Request` as the kernel text it means: the call's values and
`kept` in attributes past the IR's, a hold of the taken pools whose clauses
read `request.known` as `kept` at the first admission and `max(kept,
computed + 1)` after a preemption, the engine's records, a prefill growing
the memory pool, the first token's time, and a decode. A request to an
engine that takes nothing is its prefill and decode. So the papers' proofs
over `.hold` and `.run` stand, and the oracle and regression theorems check
that the Lean execution and the interpreter's agree. A connector (step 4),
a `prefix`, and `request.known` or `request.finished` in `cache` are outside
the fragment: the generator says so (`Fragment`).

An engine's records are written where something reads them, an
observation the theorem compares or an attribute an expression reads: one
nothing reads changes nothing a theorem states, and the papers' proofs
follow the program's text. For the same reason a request to an engine that
takes nothing, whose input is an attribute and whose first token's time
nothing reads, is a prefill of its input and a decode of the rest, without
the branch an input of 0 would need, where the interpreter decodes the
whole output. That is the interpreter's request for an input of 1 or more,
and the workloads the theorem is about have one: a claim's family requires
it (`1 ≤ w.attr i k`) only where every value the program's `init` gives the
attribute is at least 1 (`bari_rad.sq`'s `vp`, a positive multiple of 128),
so the family still holds the program's support, and the generator refuses
the program otherwise; an oracle's sessions are checked to have one.

## Step 4: an engine's KV connector

In vLLM a KV connector is one component with two sides. At a fresh
admission the scheduler asks it how many tokens it can load beyond the local
hit (`get_num_new_matched_tokens`, `scheduler.py:950-953`), allocates blocks
for them and schedules no new tokens (`scheduler.py:1069-1072`), and the
request waits for the KV in `WAITING_FOR_REMOTE_KVS`
(`scheduler.py:1264-1267`). LMCache's connector finds whole chunks
(`lmcache_mp_connector.py:774-777`) and answers what of them lies past the
local hit (`lmcache_mp_connector.py:788-792`) and nothing for a preempted request
(`lmcache_mp_connector.py:756-759`). At a request's end the connector may
keep its blocks until its save is done (`request_finished` returns `True`,
`lmcache_mp_connector.py:952`; `scheduler.py:2636-2653`).

`examples/multi-turn/vllm_lmcache.sq`:

```serq
  engine vllm on gpu {
    reqs cap max_seqs;
    tokens cap B;
    connector lmc {
      per_token (r);
      load (request.preempted ? 0 : beyond(request.known, request.hit));
      save (floor(request.input + request.output, chunk));
    }
    scheduler {
      let threshold = running.count + waiting.count > 1 ? chunk_cap : inf;
      advance running each at most (threshold);
      admit waiting while (running.preempted == 0) each at most (threshold)
        taking reqs (1),
          kv (min(request.known, request.hit + (request.loaded > 0 ? request.loaded : budget_left(vllm))))
          reserve (request.known)
        reuse (floor(request.known - 1, blocksize(kv)))
        cache (request.input + request.output);
    }
    …
  }
```

- `connector C { … }` names a queue that plays `l2`, declared above: its
  pool holds what was saved, per session, and its `serve` is the bandwidth
  the copies divide. `per_token (r)` is the time one token's copy takes,
  read as a copy starts.
- `load (n)` is read at the admission, after `request.hit`, as `taking` is.
  It is `request.loaded`, which `taking` reads: the request is given the
  blocks the copy fills and none for new work. Admitted with `n > 0`, the
  request copies `n` tokens at `C`'s bandwidth, its KV position moves by
  `n`, and it prefills the rest. While it copies it has no job at the
  engine and is no resident, as a request waiting for remote KV is in no
  `running` list, and no preemption picks it.
- `save (n)` is read when the request has generated its output, as `cache`
  is (`request.finished` 1). Before its pools go back, the request's entry
  in `C`'s pool comes to hold `n` tokens: the part the entry lacks is copied
  at `C`'s bandwidth.
- The engine records `E.loaded` for every admission, as an observation and
  as an attribute its caller reads, and `E.saved`, the tokens the L2 holds
  of the request after its save.
- `request.preempted` is 1 at the admissions after a preemption. LMCache
  loads nothing then; another connector might, and the program says which
  (criterion 2).

The two copies are the kernel's: in the IR, `CStep::connector` holds each
amount and the block it means, a load `run C (cost(C, E.loaded · r)); load P
(cost(P, E.loaded));` into the request's hold on the memory `P`, a save the
hold on `C`'s pool that takes the entry it has, grows by what it lacks,
copies it and caches the whole. The interpreter sets `E.loaded` or
`E.saved` and runs the block as the session's statements while the request
holds its pools. These are the statements `kvt.l2_load` and `kvt.l2_store`
wrote in a request's body, and both go: the copies are the connector's, no
program writes them in a hold of its own, and they are refused with the
connector's form (*Keeping the frontend small*: one meaning, one spelling).

`vllm_lmcache.sq`'s report is the one it gave before, line for line (seed 1,
horizon 3000, at 2000 blocks and at 400, where 137 preemptions bring
requests back), but for the observations' names: `l2_hit` is `vllm.loaded`,
`prefill_tokens` `vllm.prefill`, and `l2_after_preemption`, the 0 the
program checked a preempted request loaded, is the program's `load` itself
(`tests/connector.rs`).

IR 13 has no tag, so the IR version does not move. The coming tag's message
names, for steps 3 and 4: `CStmt::Request`; `CStep::request` (`CRequest`,
`RequestRecords`); `CStep::connector` (`CConnector`, whose blocks are roots
of the block tree); `Moment::Request` and `Moment::Release`; the context
variables `request.input`, `request.output`, `request.known`,
`request.hit`, `request.finished`, `request.loaded` and
`request.preempted`; and that the call's second argument is every token the
request generates, the one its prefill samples included, at least one
(`bari_rad.sq` calls `E (vp, vd + 1)` for its `vd` decode steps).

## Self-critique

**`return SchedulerOutput(…)` in the scheduler, `worker (out) { … }`.**
It would say the hand-off in the text. But the batch is what the
scheduler's statements scheduled, which the language computes; a program
that wrote it could write a `tokens` other than the tokens scheduled, two
sources for one quantity (criterion 0), and the language has no record
type to write it with. The hand-off is a check instead: the worker reads
`batch.…` and nothing else of the iteration.

**`before_execute` and `after_execute` hooks.** A hook that adds time is a
term of the forward's time, a second spelling of it; a hook of arbitrary
statements per iteration is a procedure in the engine again. The worker
takes named items, each of which says what it does to the state.

**`schedule` kept, `worker` added.** `schedule` names the act and `worker`
a component; a reader of vLLM finds `Scheduler` and the model runner, two
components. Both blocks are named for what they are.

**A connector's `load` and `save` as worker items.** Steps 1 and 2 said the
worker would take them. But the worker says how long the batch's forward
takes, from `batch.…`, and LMCache's copies are neither part of it nor of
the batch: they run at the L2's bandwidth, beside the forward, and the
tokens to copy are a request's, which the worker does not read. A worker
item would be a second place for a job on another stage, and an exception to
the worker reading the batch alone.

**`load` and `save` as clauses of the admission**, beside `reuse` and
`cache`. Fewer words, but one component's parts would be spread over
`taking`: the L2 it copies through, the time per token twice, and a `save`
clause on an admission that is read at the release. vLLM configures one
`kv_connector` per engine; the engine declares one `connector`.

**`computed == 0` for a request not yet preempted.** It is the position a
preempted request had reached, and a request preempted before its first
iteration computed anything has reached 0: its next admission would read as
a first one and load, where LMCache, reading the request's status, loads
nothing. `request.preempted` reads what LMCache reads.

**A request as parse-time sugar for a hold and its runs.** The surface
could stay (`E (input, output);` and `taking … reuse … cache`) and lower to
the hold and the `prefill` and `decode` runs the programs wrote, as the
`request` block did; the Lean generator does write that kernel text. But
the kernel cannot say all of what the engine keeps: a hold's `cache` is read
once at the release and cannot tell an end from a preemption
(`request.finished`) or read the tokens the request has by then
(`request.known`); nothing in a hold says it was preempted
(`request.preempted`); and a connector loads between the admission and the
prefill and saves before the release, which the lowering would write as
statements inside the hold that the session could also write. Kept as the
kernel, those would be new kernel words, or the frontend's lowering would
have to keep the state in the program's attributes, the five lines step 3
removes, now in trusted, unchecked code (*Keeping the frontend small*). As an
IR node the request is checked where its clauses are read (hidden
attributes, registers, the moments), and the generator's lowering for the
Lean fragment is checked against the interpreter by the oracle and
regression theorems.

**The save as the interpreter's own code.** A save waits for the L2's pool,
grows into it, may wait again when it is full, and caches; the kernel's hold
does each of these, and the Lean model has the hold. The IR carries the
block the save means, which the interpreter runs.
