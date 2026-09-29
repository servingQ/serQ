# Stage

```seq
stage NAME [ '[' N ']' ] : kind;
```

A stage is where time passes, and where a [`run`](statements.md#run) takes it.

| Kind | Servers | Unit of `run`'s work |
|---|---|---|
| [`fifo(c)`](#fifo) | `c`, one job each | clock time at rate 1 |
| [`ps(φ)`](#ps) | all jobs at once | clock time, at rate `φ(n)/n` per job |
| [`delay`](#delay) | infinite | clock time at rate 1 |
| [`step { … }`](#step) | an iterating engine | the unit of `budget` (tokens) |

The clock has no unit of its own: costs in seconds run in seconds, and
`cost 1` runs on the step clock.

## `fifo`

```seq
fifo [ ( c ) ]
```

| Argument | Type | Default | Description |
|---|---|---|---|
| `c` | `const`, a positive integer | `1` | Servers. Jobs are served in arrival order. |

## `ps`

```seq
ps ( expr )
```

| Argument | Type | Moment | Description |
|---|---|---|---|
| `expr` | `expr` | `Ps` | Total throughput `φ(n)`, shared equally by the jobs present. The [context variable](context.md) `n` is the number of jobs. |

```seq
stage link[2] : ps(1);     // a decoder's NIC: its reads share the bandwidth
```

(`examples/pd-disaggregation/llmd_pd.seq`)

## `delay`

```seq
delay
```

Every job proceeds at rate 1 with no waiting.

## `step`

```seq
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
| `budget` | `expr` | `Budget` | `inf` | Tokens per iteration. Reads `nres`, `ndec`, `kvb`, `kvp`. |
| `cost` | `expr` | `Step` | required (a parse error without it) | Clock time of the iteration. Reads `ntok`, `ndec`, `npre`, `nres`, `kvb`, `kvp`, `attn`. |
| `chunk` | `expr` | `Budget` | `0` (no cap) | Cap on one request's prefill tokens in an iteration. |
| `serve` | see below | `Serve` | `admission` | Order in which residents take tokens. At most once. |
| `memory` | `pool` | | none | The pool whose holds give `kvb` and `kvp`, and whose `preempt lifo` victims come from this stage. |

### `serve`

| Form | Meaning | IR (`CServe`) |
|---|---|---|
| `admission` | admission order (vLLM's `running` list) | `By([])` |
| `by (k1, …)` | ascending keys per resident, ties by admission order | `By(keys)` |
| `decode first` | decodes before prefills | `By([decoding ? 0 : 1])` |
| `exclusive prefill` | only the first prefilling resident while one exists; decodes stall | `ExclusivePrefill` |

Keys read `decoding`, `admission`, `remaining` and the residents' `nres`,
`ndec`, `kvb`, `kvp`, and may not draw. `serve by (remaining)` is
shortest-remaining-first; `serve by (-remaining)` is the opposite.
`exclusive prefill` is not an order and cannot be combined with one.

### Example

From `examples/multi-turn/vllm.seq`:

```seq
stage engine : step {
  budget B;
  chunk chunk_cap;
  cost c0 + max(omega + beta * (kvb + kvp), ntok * a);
  memory kv;
}
```
