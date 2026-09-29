# Statements

The kernel statements of a `session` or `server` block. Commands take no time:
they run whenever a session is ready, in the order sessions became ready. Only
[`run`](#run) lets the clock move. Every `expr` below is evaluated at the
`Session` moment unless the entry says otherwise.

The [serving vocabulary](serving.md) is sugar the parser rewrites to these.

| Statement | Does |
|---|---|
| [`set`](#set) | assigns a session attribute |
| [`observe`](#observe) | records a sample |
| [`turn`](#turn) | draws the next turn's attributes |
| [`hold`](#hold) | takes units of pools for the scope of a block |
| [`grow`](#grow) | enlarges the innermost hold |
| [`drop`](#drop) | discards the session's cached prefix |
| [`release`](#release) | gives the innermost hold's units back now |
| [`load`](#load) | the KV of some tokens arrived from outside |
| [`run`](#run) | spends work at a stage |
| [`branch`](#branch) | a test or a draw |
| [`loop`](#loop) | repeats a block |
| [`choose`](#choose) | picks an index by the smallest key |
| [`end`](#end) | ends the session |

## `set`

```seq
set NAME = expr;
```

Assigns the session attribute `NAME`. Every name assigned by `set` or `choose`
is an attribute of every session.

## `observe`

```seq
observe NAME = expr;
```

Records a sample of `expr` after warm-up, with the time, session and turn.
`--dump DIR` writes the samples; the report summarises them.

## `turn`

```seq
turn;
```

Runs the workload's `turn` block, or with a `trace` loads the next turn's
attributes and sets `more`.

## `hold`

```seq
hold POOL (units) [reserve (r)] [, POOL (units) [reserve (r)]]*
     [reuse (ρ)]
     [at admission (NAME = expr, …)]
     block
     [cache (ℓ)]
     [lease POOL (t)];
```

Joins the queue of the first pool. When the head is admitted, `units` of each
pool are allocated, the body runs, and at its end the units are released and
`min(ℓ, computed)` stay cached (rounded down to blocks).

| Argument | Type | Moment | Default | Description |
|---|---|---|---|---|
| `POOL` | `pool` | | | One or more. Admission needs room in every one; the hold waits in the first pool's queue. |
| `units` | `expr` | `Admit` | | Units to allocate at admission. |
| `reserve` | `expr` | `Admit` | `units` | Room required before admitting, `used + max(units, r) ≤ cap`. It does not change what is taken. |
| `reuse` | `expr` | `Admit` | no bound | At most this many units of the session's own cached prefix are consumed (rounded down to blocks); `cached` is set to what was. The rest stays as a dead entry until evicted. Without `reuse`, the whole own prefix is consumed. |
| `at admission` | `NAME = expr, …` | `Admit` | | Names for the header, substituted by the parser into the units, `reserve`, `reuse`, `cache` and `lease`, and set at the top of the body when the body reads it. A binding the body reads may read only attributes and constants: one of live state (`cachedin(p)`, `cached`, `now`) is a parse error, and the body reads `cached` instead. Its name is its own (not a builtin attribute, context variable, `let` or attribute the program sets) and is read only in the holds that bind it. A later binding sees earlier ones. A binding may not draw. |
| `block` | `block` | `Session` | | The body. |
| `cache` | `expr` | `Session` | `0` | Units kept cached at the end, at most what was computed. Read when the session releases. |
| `lease` | `pool`, `expr` | `Session` | none | That pool's allocation outlives the scope: neither evictable nor a preemption victim until `release` of it, `t` clock units, or the session's end; `cache` applies then. |

The header (`units`, `reserve`, `reuse`) is read at admission, not when the
session queues. A `set` above the hold is read when the session reaches it, so
a value it takes from live pool or stage state is stale by admission, and
linking rejects a header that reads one. Name the value with `at admission`
instead.

If the hold is preempted (`preempt lifo`) it re-enters the head of the queue
and the statement executes again with `computed` set to the position it had
reached.

## `grow`

```seq
grow POOL (d);
```

| Argument | Type | Description |
|---|---|---|
| `POOL` | `pool` | A pool the session holds. |
| `d` | `expr` | Units added to the innermost hold on `POOL`, rounded to blocks. |

If it does not fit, the pool's [`preempt`](pool.md#preempt) applies.

## `drop`

```seq
drop POOL;
```

Discards the session's own cached prefix in `POOL`. `end` keeps the session's
cached prefixes (the cache does not know a session has left), so a program that
models dropping them writes this before `end`.

## `release`

```seq
release POOL;
```

The innermost enclosing hold on `POOL` gives its allocation back now, caching
per its `cache`, and holds `POOL` no longer. Outside any hold on `POOL` it ends
a lease of `POOL`. A session that holds and leases nothing there releases
nothing.

## `load`

```seq
load POOL (n);
```

| Argument | Type | Description |
|---|---|---|
| `POOL` | `pool` | |
| `n` | `expr` | Tokens of KV that arrived from outside the engine. |

The innermost enclosing hold's computed position on `POOL` advances by `n`,
within its allocation; a program that needs more grows first. Afterwards `cache`
and `cached` count them.

## `run`

```seq
run STAGE [prefill | decode] (work) [growing POOL];
```

| Argument | Type | Description |
|---|---|---|
| `STAGE` | `stage` | Indexed when the stage is an array. |
| mode | `prefill` \| `decode` | Required on a `step` stage, forbidden on any other. |
| `work` | `expr` | Clock time at rate 1 (`fifo`, `delay`), or at `φ(n)/n` (`ps`), or tokens on a `step` stage. A run of zero work completes at once. |
| `growing` | `pool` | Only on a `step` stage. The hold on this pool grows block by block as the run advances, preempting if needed. |

Blocks the session until the work is done.

## `branch`

```seq
branch (test) block [else block]
branch with (p) block [else block]
```

| Form | Argument | Takes the first block |
|---|---|---|
| `branch (test)` | `expr`, must be 0 or 1 | when `test` is 1 |
| `branch with (p)` | `expr`, a probability | with probability `p`; sugar for `branch (~bernoulli(p))` |

Any other value of `test` (a fraction, a count, a negative, NaN) is a run-time
error, and a constant strictly between 0 and 1 is refused at link time as a
draw written as a test.

## `loop`

```seq
loop block
```

Repeats the block until an `end`.

## `choose`

```seq
choose NAME in n by (key);
```

| Argument | Type | Description |
|---|---|---|
| `NAME` | identifier | Becomes a session attribute. |
| `n` | `expr` | Number of candidates, `0 … n-1` (rounded down; 0 or less leaves `NAME` at 0). |
| `key` | `expr` | Evaluated with `NAME` bound to each candidate. |

`NAME` is set to the index with the smallest key, ties to the smallest index.
Used with an array of stages: `choose j in 2 by (work(prefill[j]));`.

## `end`

```seq
end;
```

The session leaves. It releases every hold but keeps its cached prefixes.
