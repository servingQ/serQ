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
  serve admission;  |  serve by (expr, …);  |  serve decode first;  |  serve exclusive prefill;
  serve only (expr) [admission | by (expr, …) | decode first];
  memory POOL;
  iteration { stmt … }      // stmt: serve […]; | admit [while (expr)]; | branch (expr) { … } [else { … }]
}
```

An engine that runs iterations (continuous batching). An iteration serves its
residents one token per decoding job and up to `chunk` per prefilling job until
`budget` is spent. A `growing` job first grows its hold to the position it will
reach. Then the stage admits from the pools that name it in [`admit via`](pool.md#admit-via). The
iteration lasts `cost`, and its tokens are applied when it ends. An iteration
that schedules no token is not one, unless it preempted.

| Clause | Type | Moment | Default | Description |
|---|---|---|---|---|
| `budget` | `expr` | `Budget` | `inf` | Tokens per iteration. Reads `residents`, `decoders`, `kv_decode`, `kv_prefill`. |
| `cost` | `expr` | `Step` | required (a parse error without it) | Clock time of the iteration. Reads `tokens`, `decoders`, `prefilled`, `residents`, `kv_decode`, `kv_prefill`, `attention`. |
| `chunk` | `expr` | `Budget` | `0` (no cap) | Cap on one request's prefill tokens in an iteration. |
| `serve` | see below | `Serve` | `admission` | Which residents are served (`only`) and in what order, or an exclusive-prefill batch policy. At most once. |
| `memory` | `pool` | | none | The pool whose holds give `kv_decode` and `kv_prefill`, and whose `preempt lifo` victims come from this stage. |
| `iteration` | a body | `Serve`, `Plan` | vLLM's procedure | The iteration as the program writes it: whom it serves, in what order, and when it admits (below). Not with `exclusive prefill` or `only`. |

### `serve`

| Form | Meaning | IR (`CServe`) |
|---|---|---|
| `admission` | admission order (vLLM's `running` list) | `By([])` |
| `by (k1, …)` | ascending keys per resident, ties by admission order | `By(keys)` |
| `decode first` | decodes before prefills | `By([decoding ? 0 : 1])` |
| `exclusive prefill` | one prefill alone, or a decode-only batch; a fitting waiting prefill displaces tentative resident decodes | `ExclusivePrefill` |
| `only (p)` then an order | only the residents for which `p` is nonzero, in that order (`admission` when none is written) | `CStep.only = Some(p)` beside the order's `By` |

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
fit, queue-head and no-admission-after-preemption gates still apply. This
policy does not supply vendor PP caps or remote-KV admission rules. See
[Separate prefill/decode batches](../design/exclusive-prefill.md).

`only (p)` is read for each resident at its turn, from the variables a key
reads, the totals as the residents stand at that read (a session the
iteration admitted included). Unlike a key it may not read `now` or
`work(…)`: an engine whose residents it all excludes waits for the next
event, and the clock moving is none. It may not draw. A resident it
excludes gets no token this iteration, keeps its allocation and advances no
computed KV; an admitted session it excludes waits as such a resident.
`serve only (decoders > 0 ? decoding : !decoding);` is FasterTransformer's decode-only batches
([FasterTransformer](../use-cases/fastertransformer.md)). `only` does not
combine with `exclusive prefill`. See
[Serving a subset](../design/serve-only.md).

### `iteration`

```serq
iteration {
  serve [only (p)] [admission | by (k, …) | decode first];
  admit [while (e)];
  branch (e) { … } [else { … }]
}
```

The iteration as the program writes it, in place of the procedure a stage
without one runs. The statements run once each, in order; a body has no
loop, so an iteration ends. A resident is served at most once per
iteration.

| Statement | What it does |
|---|---|
| `serve [only (p)] [order]` | Gives the residents not yet served their tokens (one to a decode, up to `chunk` to a prefill, a `growing` job growing first) in the order (the stage's `serve` order when none), while budget is left. A resident `p` reads as 0 is skipped and stays unserved, for a later `serve`. A grower that preempts itself ends the statement. |
| `admit [while (e)]` | Admits the head of the queues that name this stage in `admit via` and serves the newcomer, one at a time, while budget is left, the head fits and `e` (read before each) is 1. |
| `branch (e) { … } else { … }` | A test: the first body when `e` is 1, the second when it is 0. |

A guard and a `while` are read at the `Plan` moment: the residents' totals
(`residents`, `decoders`, `kv_decode`, `kv_prefill`) as they stand, what the
iteration has scheduled so far (`tokens`, `prefilled`), `admitted` (the
sessions it has admitted) and `preempted` (1 once it has preempted), pool
and stage queries and constants; neither draws, reads `now`, `work(…)` or
`budget_left(…)`. `only` and keys read what a `serve` key reads. A body with
a path that neither serves nor admits does not link: an engine that took it
would schedule nothing and wait for an event that may never come.

A stage without a body runs vLLM's procedure, which is this body
(`tests/iteration_body.rs` runs every example both ways):

```serq
iteration { serve; admit while (!preempted); }
```

Other engines' are other bodies:

| Engine | Body |
|---|---|
| SGLang (no mixed chunk): the chunked request and new prefills alone, a decode batch when no prefill forms | `iteration { serve only (!decoding); admit; branch (tokens == 0) { serve; } }` |
| TensorRT-LLM `STATIC_BATCH`: admit only into an empty engine | `iteration { serve; branch (residents == 0) { admit; } }` |
| FasterTransformer as Dai et al. model it | `iteration { branch (decoders > 0) { serve only (decoding); } else { serve; admit; } }` |

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
