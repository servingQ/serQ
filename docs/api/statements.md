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

```serq
set NAME = expr;
```

Assigns the session attribute `NAME`. Every name assigned by `set` or `choose`
is an attribute of every session.

## `observe`

```serq
observe NAME = expr;
```

Records a sample of `expr` after warm-up, with the time, session and turn.
`--dump DIR` writes the samples; the report summarises them. A test (an
expression whose outermost operator is a comparison, `&&`, `||` or `!`) that
was 0 over 40 or more samples gets a line under the table, `note: observe hit
is constant 0 over 5357 samples`; the note says the run never varied that
value, which is the program's intent (`examples/single-turn/vllm_single_turn.sq`
never reads its cache back) or a bug to find before reading the means.

## `turn`

```serq
turn;
```

Runs the workload's `turn` block, or with a `trace` loads the next turn's
attributes and sets `more`.

## `hold`

```serq
hold POOL (units) [reserve (r)] [, POOL (units) [reserve (r)]]*
     [reuse (ρ)]
     [at admission (NAME = expr, …)]
     block
     [cache (ℓ)]
     [lease POOL (t)];
```

Joins the queue of the first pool. When the head is admitted, `units` of each
pool are allocated, the body runs, and at its end the units are released and
`min(ℓ, computed)` stay cached (rounded down to blocks). The `cache` clause is
what makes the hold take part in the prefix cache: with it the admission
consumes the session's own cached prefix (`cached`); without it the hold
leaves the session's cached blocks where they are and `cached` is 0 in its
body (a body that reads it there, or a `reuse` there, does not link).
`cache (0)` consumes the prefix and keeps nothing.

| Argument | Type | Moment | Default | Description |
|---|---|---|---|---|
| `POOL` | `pool` | | | One or more. Admission needs room in every one; the hold waits in the first pool's queue. |
| `units` | `expr` | `Admit` | | Units to allocate at admission. |
| `reserve` | `expr` | `Admit` | `units` | Room required before admitting, `used + max(units, r) ≤ cap`. It does not change what is taken. |
| `reuse` | `expr` | `Admit` | no bound | At most this many units of the session's own cached prefix are consumed (rounded down to blocks); `cached` is set to what was. The rest stays as a dead entry until evicted. Without `reuse`, the whole own prefix is consumed. Only with `cache`: a hold without the clause consumes nothing. |
| `at admission` | `NAME = expr, …` | `Admit` | | Names for the header, substituted by the parser into the units, `reserve`, `reuse`, `cache` and `lease`, and set at the top of the body when the body reads it. A binding the body reads may read only attributes and constants: one of live state (`cachedin(p)`, `cached`, `now`) is a parse error, and the body reads `cached` instead. Its name is its own (not a builtin attribute, context variable, `let` or attribute the program sets) and is read only in the holds that bind it. A later binding sees earlier ones. A binding may not draw. |
| `block` | `block` | `Session` | | The body. |
| `cache` | `expr` | `Session` | none | Units kept cached at the end, at most what was computed. Read when the session releases. Its presence is what makes the hold consume the session's prefix at admission; `cache (0)` consumes and keeps nothing, no clause leaves the prefix where it is. |
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

```serq
grow POOL (d);
```

| Argument | Type | Description |
|---|---|---|
| `POOL` | `pool` | A pool the session holds. |
| `d` | `expr` | Units added to the innermost hold on `POOL`, rounded to blocks. |

If it does not fit, the pool's [`preempt`](pool.md#preempt) applies.

## `drop`

```serq
drop POOL;
```

Discards the session's own cached prefix in `POOL`. `end` keeps the session's
cached prefixes (the cache does not know a session has left), so a program that
models dropping them writes this before `end`.

## `release`

```serq
release POOL;
```

The innermost enclosing hold on `POOL` gives its allocation back now, caching
per its `cache`, and holds `POOL` no longer. Outside any hold on `POOL` it ends
a lease of `POOL`. A session that holds and leases nothing there releases
nothing.

## `load`

```serq
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

```serq
run STAGE [prefill | decode] (work) [growing POOL];
run STAGE, STAGE [, STAGE]* (work);
```

| Argument | Type | Description |
|---|---|---|
| `STAGE` | `stage` | Indexed when the stage is an array. |
| mode | `prefill` \| `decode` | Required on a `step` stage, forbidden on any other. |
| `work` | `expr` | Clock time at rate 1 (`fifo`, `delay`), or at `φ(present)/present` (`ps`), or tokens on a `step` stage. A run of zero work completes at once. |
| `growing` | `pool` | Only on a `step` stage. The hold on this pool grows block by block as the run advances, preempting if needed. |

Blocks the session until the work is done.

With several stages the run is one job, a *flow*, that holds all of them
from its start to its end; its work goes down at one rate everywhere, which
the program's [`share`](program.md#share) sets from the stages'
capacities. Every stage is `ps(φ)` with a constant `φ` above 0, a run names each
stage array once, and the work is in the unit the capacities share (tokens
at tokens per second, say). A stage array some run holds with another is
*shared* for the whole run: every job on it, a single-stage run included,
is a flow, and the report's utilisation is the capacity its flows carry.

```serq
stage egress[2] : ps(BwP);     // a prefiller's NIC, tokens per second
stage ingress[2] : ps(BwD);    // a decoder's NIC
share maxmin;
…
run egress[i], ingress[j] (tokens);
```

## `branch`

```serq
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

```serq
loop block
```

Repeats the block until an `end`.

## `choose`

```serq
choose NAME in n by (key, …);
```

| Argument | Type | Description |
|---|---|---|
| `NAME` | identifier | Becomes a session attribute. |
| `n` | `expr` | Number of candidates, `0 … n-1` (rounded down; 0 or less leaves `NAME` at 0). |
| `key, …` | `expr`, one or more | Evaluated with `NAME` bound to each candidate. Several keys compare in order: the second decides among the first's ties, and so on. |

`NAME` is set to the index with the smallest key (tuple), ties to the smallest index.
Used with an array of stages: `choose j in 2 by (work(prefill[j]));`.

## `end`

```serq
end;
```

The session leaves. It releases every hold but keeps its cached prefixes.
