# Stage

```serq
stage NAME [ '[' N ']' ] : kind;
```

A stage is where time passes, and where a [`run`](statements.md#run) takes it.

| Kind | Servers | Unit of `run`'s work |
|---|---|---|
| [`fifo(c)`](#fifo) | `c`, one job each | clock time at rate 1 |
| [`ps(φ)`](#ps) | all jobs at once | clock time, at rate `φ(present)/present` per job |
| [`delay`](#delay) | infinite | clock time at rate 1 |
| [`step { … }`](#step) | an iterating engine | the unit of `budget` (tokens) |

The clock has no unit of its own: costs in seconds run in seconds, and
`cost 1` runs on the step clock.

## `fifo`

```serq
fifo [ ( c ) ]
```

| Argument | Type | Default | Description |
|---|---|---|---|
| `c` | `const`, a positive integer | `1` | Servers. Jobs are served in arrival order. |

## `ps`

```serq
ps ( expr )
```

| Argument | Type | Moment | Description |
|---|---|---|---|
| `expr` | `expr` | `Ps` | Total throughput `φ(present)`, shared equally by the jobs present. The [context variable](context.md) `present` is the number of jobs. |

```serq
stage svc : ps(1);
```

(`examples/single-turn/ps.sq`, M/G/1-PS.) A `ps` stage some run holds
together with another is served by the program's
[`share`](program.md#share) instead: its jobs are flows, not an equal
split.

## `delay`

```serq
delay
```

Every job proceeds at rate 1 with no waiting.

## `step`

```serq
step {
  budget expr;
  cost expr;
  chunk expr;
  granule c;               // a prefill gets all it has left or a multiple of c (inf: whole)
  serve admission;  |  serve by (expr, …);  |  serve decode first;  |  serve exclusive prefill;
  serve only (expr) [admission | by (expr, …) | decode first];
  memory POOL;
  state NAME = c;           // a register the body sets
  iteration { stmt … }      // serve, admit, branch and set (below)
}
```

An engine that runs iterations (continuous batching). By default, an iteration serves its
residents one token per decoding job and up to `chunk` per prefilling job until
`budget` is spent. A `growing` job first grows its hold to the position it will
reach. Then the stage admits from the pools that name it in [`admit via`](pool.md#admit-via). The
iteration lasts `cost`, and its tokens are applied when it ends. An attempt that schedules no token and preempts nobody waits for the next
event without running a timed iteration.

| Clause | Type | Moment | Default | Description |
|---|---|---|---|---|
| `budget` | `expr` | `Budget` | `inf` | Tokens per iteration. Reads `residents`, `decoders`, `kv_decode`, `kv_prefill`. |
| `cost` | `expr` | `Step` | required (a parse error without it) | Clock time of the iteration. Reads `tokens`, `decoders`, `prefilled`, `residents`, `kv_decode`, `kv_prefill`, `attention`. |
| `chunk` | `expr` | `Budget` | `0` (no cap) | Cap on one request's prefill tokens in an iteration. |
| `granule` | `const`, positive | | none | Prefill chunk alignment; see below. |
| `serve` | see below | `Serve` | `admission` | Which residents are served (`only`) and in what order, or an exclusive-prefill batch policy. At most once. |
| `memory` | `pool` | | none | The pool whose holds give `kv_decode` and `kv_prefill`, and whose preemption victims come from this stage. |
| `iteration` | a body | `Serve`, `Plan` | serve, then admit | The iteration as the program writes it: whom it serves, in what order, and when it admits (below). Not with `exclusive prefill` or `only`. |

### `granule`

A prefill receives all its remaining work, or a multiple of `granule`.
After `chunk` caps the work, it is rounded down to that multiple; a result
of 0 waits. `granule inf` schedules a whole prefill or none. A waiting
prefill stays resident: later residents may still be served, but the
iteration admits no further requests.

The constant must be positive, cannot exceed a positive constant `chunk`, and cannot
be combined with `exclusive prefill`.

### `serve`

| Form | Meaning |
|---|---|
| `admission` | admission order |
| `by (k1, …)` | ascending keys per resident, ties by admission order |
| `decode first` | decodes before prefills |
| `exclusive prefill` | one prefill alone, or a decode-only batch |
| `only (p)` followed by an optional order | serve only residents for which `p` is nonzero; default order is `admission` |

Keys read `decoding`, `admission`, `remaining` and the totals `residents`,
`decoders`, `kv_decode`, `kv_prefill`, and may not draw. `serve by (remaining)` is
shortest-remaining-first; `serve by (-remaining)` is the opposite.
`exclusive prefill` is not an order and cannot be combined with one. A
resident prefill takes precedence and runs alone. Otherwise residents are
considered for decode; while budget is left, a fitting waiting prefill can
replace that selection and use the full budget. A selected prefill admits
no further waiting request in that iteration. Cancelled decode work neither
runs nor advances computed KV; any capacity already allocated remains held.
During these admissions `budget_left` supplies the full budget. Ordinary
fit, queue-head and no-admission-after-preemption gates still apply.

`only (p)` is read for each resident at its turn, from the variables a key
reads, the totals as the residents stand at that read (a session the
iteration admitted included). Unlike a key it may not read `now` or
`work(…)`: an engine whose residents it all excludes waits for the next
event, and the clock moving is none. It may not draw. A resident it
excludes gets no token this iteration, keeps its allocation and advances no
computed KV; an admitted session it excludes waits as such a resident.
`serve only (decoders > 0 ? decoding : !decoding);` is FasterTransformer's decode-only batches
([FasterTransformer](../use-cases/fastertransformer.md)). `only` does not
combine with `exclusive prefill`.

### `iteration`

```serq
iteration {
  serve [only (p)] [admission | by (k, …) | decode first];
  admit [only (p)] [while (e)];
  branch (e) { … } [else { … }]
  set NAME = e;
}
```

The iteration as the program writes it, in place of the procedure a stage
without one runs. The statements run once each, in order; a body has no
loop, so an iteration ends. A resident is served at most once per
iteration.

| Statement | What it does |
|---|---|
| `serve [only (p)] [order]` | Gives the residents not yet served their tokens (one to a decode, up to `chunk` to a prefill, a `growing` job growing first) in the order (the stage's `serve` order when none), while budget is left. A resident `p` reads as 0 is skipped and stays unserved, for a later `serve`. A grower that preempts itself ends the statement. |
| `admit [only (p)] [while (e)]` | Admits the head of the queues that name this stage in `admit via` and serves the newcomer, one at a time, while budget is left, the head fits and `e` (read before each) is 1. A newcomer for which `p` is 0 is admitted and waits unserved. |
| `branch (e) { … } else { … }` | A test: the first body when `e` is 1, the second when it is 0. |
| `set NAME = e` | Updates one of this stage's registers; see below. |

Guards and `while` conditions are read at `Plan`: current resident totals,
tokens scheduled so far (`tokens`, `prefilled`), `admitted`, `preempted`,
pool and stage queries, and constants. They cannot draw or read `now`,
`work(…)` or this stage's `budget_left(…)`. Keys and `only` predicates follow
the rules of `serve`.

Every path through a body must reach a `serve` or `admit`; otherwise it is a
link error. This does not guarantee progress: `admit while (tokens > 0)`
cannot admit into an empty engine. If the run ends with residents or a
waiting queue and the last iteration attempt scheduled nothing, the report
flags `idle: stage …` (`idle_with_work` in JSON).

Without an explicit body, the stage serves residents and admits only if no
preemption occurred:

```serq
iteration { serve; admit while (!preempted); }
```

To admit only into an empty engine:

```serq
iteration {
  serve;
  branch (residents == 0) { admit; }
}
```

#### Registers

`state NAME = c;` declares a numeric register initialized by constant `c`.
The iteration body updates it with `set NAME = e;`, where `e` follows guard
expression rules. Values persist between iterations. Updates are undone
if the attempt schedules no token, preempts nobody and admits nobody.

A register may be read in:

- its stage's budget, chunk, cost, serve keys, body and iteration claims;
- queue keys of pools admitted by that stage;
- hold headers whose first pool is admitted by that stage;
- gauges and `at end` claims.

Other reads are link errors. Each register name must be unique and cannot
also name an attribute, constant, context variable, pool or stage. Stage
arrays cannot declare registers, and a body can set only its own stage's
registers.

A guard based on a register must allow recovery when the engine empties.
For example, this policy skips admission after an iteration that admitted,
unless no residents remain:

```serq
state just = 0;
iteration {
  serve;
  branch (just == 0 || residents == 0) { admit; }
  set just = admitted > 0;
}
```

Without the `residents == 0` condition, an empty engine with `just == 1`
could never admit again: its unsuccessful attempt would undo the reset.
The report flags an engine left in this state as `idle: stage …`.

### Example

From `examples/multi-turn/vllm.sq`:

```serq
stage engine : step {
  budget B;
  chunk long_prefill(reqs, chunk_cap);
  cost c0 + max(omega + beta * (kv_decode + kv_prefill), tokens * a);
  memory kv;
}
```

## Examples

A complete program:

```serq
fn main() {
  stage engine : step { budget 8; cost 1; }
  workload {
    arrive batch(2);
    session { request; end; }
  }
  server {
    run engine prefill (8);
    run engine decode (2);
    observe finished = now;
  }
}
```

Save as `model.sq`, then run:

```sh
serq run model.sq --horizon 10
```

## See also

[`run`](statements.md#run), [context variables](context.md),
[`pyserq.Stage`](../python/stage.md).
