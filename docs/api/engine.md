# Engine

```serq
device NAME [ '[' N ']' ] { RESOURCE (PARAM, …) = expr; CAP cap expr; … }
engine NAME [ '[' N ']' ] on DEVICE {
  CAP cap expr;              // a capacity the engine holds, as a pool `on NAME`
  CAP[N] cap expr;           // N of them: N queues the one engine admits
  tokens cap expr;           // required: the tokens one iteration computes (inf: no cap)
  granule c;                 // a prefill gets all it has left or a multiple of c
  state NAME = c;            // a register the schedule sets
  schedule { stmt … }        // required: what each iteration does (below)
  execute (expr);            // required: how long the iteration takes
}
pool NAME on DEVICE { … }          // the device's capacity: the engine's memory
pool NAME on ENGINE.DEVICE { … }   // the same, admitted by the engine
pool NAME on ENGINE { … }          // the engine's capacity, admitted by it
```

An engine runs iterations (continuous batching) on a device. Each
iteration gives the running requests their tokens, one to a decoding run
and a chunk to a prefilling one, then admits waiting requests while the
budget lasts, and lasts what `execute` says. A `growing` run first grows
its hold to the position it will reach. An attempt that schedules no token
and preempts nobody waits for the next event without running a timed
iteration. A [`run E prefill` or `run E decode`](statements.md#run) puts
work on engine `E`; the work is in tokens.

An engine cannot be named `engine`: it is named for what it models (`vllm`,
`sglang`, `tgi`) or `llm`. Inside a [`queue`](../language.md#queues), `device
gpu` is the member's and `engine on gpu` is the queue's engine, named after
the queue. One device runs one engine.

The form is parse-time sugar for the IR's step stage
([Engines on devices](../language.md#engines-on-devices) maps each clause);
the [IR](../ir.md) and the Lean model see only that.

## `device`

| Item | Description |
|---|---|
| `RESOURCE (PARAM, …) = expr;` | A time resource: a demand to time, read only in the engine's `execute`, as a `def` is. Its body reads every parameter. |
| `CAP cap expr;` | A capacity, which a `pool CAP on DEVICE { … }` declares with its rules. |

## Items of an engine

| Item | Type | Read | Description |
|---|---|---|---|
| `CAP cap` | `expr` | | A capacity the engine holds (`reqs cap max_seqs`, vLLM's `max_num_seqs`), declared with `pool CAP on ENGINE`. |
| `CAP[N] cap` | `expr`, `N` a constant | | N capacities of one kind, each `cap expr`: `pool CAP on ENGINE` is the N pools `CAP[0]`…`CAP[N-1]`, each a queue the engine admits, tried in index order. An engine family (`engine E[M]`) holds none: `CAP cap` is already one per member, and N pools for M engines join neither one for one nor one for all. |
| `tokens cap` | `expr` | as the iteration starts | Tokens per iteration (vLLM's `max_num_batched_tokens`). Reads `running.count`, `running.decoding`, `running.kv_decode`, `running.kv_prefill` and `waiting.count`, not `batch.…`. |
| `granule` | `const`, positive | | Prefill chunk alignment; see below. |
| `state NAME = c` | `const` | | A register; see below. |
| `schedule` | statements | as the iteration is planned | Whom the iteration serves, in what order, and when it admits. |
| `execute` | `expr` | after the batch is formed | Clock time of the iteration. Reads `batch.…`, `running.count`, `waiting.count` and the device's time resources. |

The values an engine reads, and in which clause, are in
[Engines on devices](../language.md#engines-on-devices): `running.…` is
the running requests as they stand, `waiting.…` the queues the engine
admits, `batch.…` the iteration's batch.

## Pools on a device or an engine

A pool on a device or an engine takes its name and its capacity from it;
its body holds the rules ([Pool](pool.md)). Who admits a request waiting
for it is said where the pool is:

| Pool | Admitted |
|---|---|
| `pool reqs on llm` | by engine `llm`, in an iteration's `admit waiting` |
| `pool kv on llm.gpu` | by engine `llm`, which runs on `gpu` |
| `pool kv on gpu` | as soon as it fits |

A pool on an engine is a family as the engine is, or as its `CAP[N]` is:
`reqs[2] cap 4;` in `engine llm` and `pool reqs on llm { }` give `reqs[0]`
and `reqs[1]`, both admitted by `llm`, and a hold picks one with a computed
index (`hold reqs[t] (…)`).

The pool option `admit via ENGINE;` is refused: who admits a pool is said
where the pool is, in its `on`.

The pool on the engine's device is the engine's memory either way: its
holds give `running.kv_…` and `batch.kv_…`, and a growth that does not fit
preempts among the engine's running requests.

## `schedule`

```serq
schedule {
  let NAME = expr;                                        // read once, as the iteration starts
  advance running [only (p)] [admission | by (k, …) | decode first] [each at most (c)];
  admit waiting [only (p)] [while (e)] [each at most (c)];
  exclusive prefill [each at most (c)];
  branch (e) { … } [else { … }]
  set NAME = e;
}
```

The statements run once each, in order; a schedule has no loop, so an
iteration ends. A running request is served at most once per iteration.

| Statement | What it does |
|---|---|
| `advance running [only (p)] [order]` | Gives the running requests not yet served their tokens, in the order (admission order when none), while budget is left. A request `p` reads as 0 is skipped and stays unserved, for a later statement. A grower that preempts itself ends the statement. |
| `admit waiting [only (p)] [while (e)]` | Admits the head of the queues the engine admits and serves the newcomer, one at a time, while budget is left, the head fits and `e` (read before each) is 1. A newcomer for which `p` is 0 is admitted and waits unserved. |
| `exclusive prefill` | One prefill alone, or a decode-only batch; see below. |
| `branch (e) { … } else { … }` | A test: the first statements when `e` is 1, the second when it is 0. |
| `set NAME = e` | Updates one of this engine's registers; see below. |

vLLM's schedule serves the running requests, then admits the waiting
while nothing was preempted (`scheduler.py:624`, `scheduler.py:869`):

```serq
schedule { advance running; admit waiting while (running.preempted == 0); }
```

Guards and `while` conditions read the running requests as they stand, the
tokens scheduled so far (`batch.tokens`, `batch.prefilled`),
`waiting.admitted`, `running.preempted`, pool and stage queries, and
constants. They cannot draw or read `now`, `work(…)` or this engine's
`budget_left(…)`. Every path through a schedule must reach an `advance
running` or an `admit waiting`; otherwise it is a link error. This does
not guarantee progress: `admit waiting while (batch.tokens > 0)` cannot
admit into an empty engine. If the run ends with running or waiting
requests and the last attempt scheduled nothing, the report flags `idle:
stage …` (`idle_with_work` in JSON).

### Orders and `only`

| Order | Meaning |
|---|---|
| `admission` | admission order (the default) |
| `by (k1, …)` | ascending keys per request, ties by admission order |
| `decode first` | decodes before prefills |

Keys read `decoding`, `admission`, `remaining` and `running.count`,
`running.decoding`, `running.kv_…`, and may not draw. `by (remaining)` is
shortest-remaining-first; `by (-remaining)` is the opposite.

`only (p)` is read for each request at its turn, on the running requests
as they stand at that read (one the iteration admitted included). Unlike
a key it may not read `now` or `work(…)`: an engine whose requests it all
excludes waits for the next event, and the clock moving is none. It may
not draw. A request it excludes gets no token this iteration, keeps its
allocation and advances no computed KV. A predicate both statements read
is named with a `def`, read where it stands
([FasterTransformer](../use-cases/fastertransformer.md)):

```serq
def in_phase() { running.decoding > 0 ? decoding : !decoding }
…
schedule {
  advance running only (in_phase());
  admit waiting only (in_phase()) while (running.preempted == 0);
}
```

### `each at most`

`each at most (c)` caps one request's tokens in the iteration; one cap per
run for the whole iteration, so every statement that has one says the
same. `inf` is no cap. `c` chooses among constants (its condition may
read the running requests as the iteration starts, through a `let`), and
one at or below 0 does not link. A cap that follows the iteration is
written `max(k, e)` with `k` a constant above 0: `e` reads what the
condition reads, and `k` keeps every run above 0. vLLM's adaptive
threshold (`scheduler.py:609-622`), with `threshold` and `budget` the
program's constants, is

```serq
let n = running.count + waiting.count;
let c = n > 1 ? max(threshold, floor(budget / n)) : inf;
```

It is vLLM's only when `budget` is the engine's `tokens cap`, vLLM's
`max_num_batched_tokens` (`scheduler.py:580`). `max(0, e)`, `min(k, e)`
or a bare `e` does not link; a cap that shrinks writes its floor,
`max(1, min(k, e))`.

### `exclusive prefill`

```serq
schedule {
  exclusive prefill [each at most (c)];
  admit waiting while (running.preempted == 0) [each at most (c)];
}
```

A running prefill takes precedence and runs alone. Otherwise running
requests are considered for decode; while budget is left, a fitting
waiting prefill can replace that selection and use the full budget. A
selected prefill admits no further waiting request in that iteration.
Cancelled decode work neither runs nor advances computed KV; any capacity
already allocated remains held. During these admissions `budget_left`
supplies the full budget. Ordinary fit, queue-head and
no-admission-after-preemption gates still apply. It appears only in the
form above: it takes back decodes already chosen, which another schedule
cannot ([RBLN](../use-cases/rbln.md)).

### `granule`

A prefill receives all its remaining work, or a multiple of `granule`.
After `each at most` caps the work, it is rounded down to that multiple;
a result of 0 waits. `granule inf` schedules a whole prefill or none. A
waiting prefill stays running: later requests may still be served, but
the iteration admits no further requests. The constant must be positive,
cannot exceed a constant `each at most` or the floor `k` of a
`max(k, e)` one, and cannot be combined with
`exclusive prefill`.

### Registers

`state NAME = c;` declares a numeric register initialized by constant `c`.
The schedule updates it with `set NAME = e;`, where `e` follows the guards'
rules. Values persist between iterations. Updates are undone if the
attempt schedules no token, preempts nobody and admits nobody.

A register may be read in:

- its engine's `tokens cap`, `execute`, keys, schedule and iteration claims;
- queue keys of pools the engine admits;
- hold headers whose first pool the engine admits;
- gauges and `at end` claims.

Other reads are link errors. Each register name must be unique and cannot
also name an attribute, constant, context variable, pool or stage. Engine
families cannot declare registers.

A guard based on a register must allow recovery when the engine empties.
TGI's engine skips admission after an iteration that admitted, unless no
request is running (`examples/engines/tgi.sq`):

```serq
state just = 0;
schedule {
  advance running;
  branch (just == 0 || running.count == 0) { admit waiting; }
  set just = waiting.admitted > 0;
}
```

Without the `running.count == 0` condition, an empty engine with `just ==
1` could never admit again: its unsuccessful attempt would undo the reset.
The report flags an engine left in this state as `idle: engine …`.

## Examples

`examples/multi-turn/vllm.sq`'s engine:

```serq
device gpu {
  compute (t) = t * a;
  hbm (k) = omega + beta * k;
  kv cap blocks * bs;
}
engine vllm on gpu {
  reqs cap max_seqs;
  tokens cap B;
  schedule {
    let threshold = running.count + waiting.count > 1 ? chunk_cap : inf;
    advance running each at most (threshold);
    admit waiting while (running.preempted == 0) each at most (threshold);
  }
  execute (c0 + max(hbm(batch.kv_decode + batch.kv_prefill), compute(batch.tokens)));
}
pool kv on gpu { block bs; evict lru; preempt lifo; }
pool reqs on vllm { queue fifo; }
```

A complete program:

```serq
fn main() {
  device gpu { }
  engine llm on gpu {
    tokens cap 8;
    schedule { advance running; admit waiting while (running.preempted == 0); }
    execute (1);
  }
  workload {
    arrive batch(2);
  }
  server {
    run llm prefill (cost(llm, 8));
    run llm decode (cost(llm, 2));
    observe finished = now;
  }
}
```

Save as `model.sq`, then run:

```sh
serq run model.sq --horizon 10
```

## See also

[Stage](stage.md), [`run`](statements.md#run), [Pool](pool.md),
[Engines on devices](../language.md#engines-on-devices),
[`pyserq.Stage`](../python/stage.md).
