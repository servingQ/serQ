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
  memory POOL;
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
| `serve` | see below | `Serve` | `admission` | Order in which residents take tokens. At most once. |
| `memory` | `pool` | | none | The pool whose holds give `kv_decode` and `kv_prefill`, and whose `preempt lifo` victims come from this stage. |

### `serve`

| Form | Meaning | IR (`CServe`) |
|---|---|---|
| `admission` | admission order (vLLM's `running` list) | `By([])` |
| `by (k1, …)` | ascending keys per resident, ties by admission order | `By(keys)` |
| `decode first` | decodes before prefills | `By([decoding ? 0 : 1])` |
| `exclusive prefill` | only the first prefilling resident while one exists; decodes stall | `ExclusivePrefill` |

Keys read `decoding`, `admission`, `remaining` and the totals `residents`,
`decoders`, `kv_decode`, `kv_prefill`, and may not draw. `serve by (remaining)` is
shortest-remaining-first; `serve by (-remaining)` is the opposite.
`exclusive prefill` is not an order and cannot be combined with one.

### Example

From `examples/multi-turn/vllm.sq`:

```serq
stage engine : step {
  budget B;
  chunk chunk_cap;
  cost c0 + max(omega + beta * (kv_decode + kv_prefill), tokens * a);
  memory kv;
}
```
