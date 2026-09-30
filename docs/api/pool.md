# Pool

```serq
pool NAME [ '[' N ']' ] {
  cap expr;
  block expr;
  evict lru;  |  evict by (expr, …);
  preempt none;  |  preempt lifo;
  queue fifo;  |  queue by (key, …);
  admit via STAGE;
  spill POOL via STAGE (expr) when (expr);
}
```

A pool holds *units* (tokens, blocks, slots) for the sessions that
[`hold`](statements.md#hold) it, keeps the prefixes they release as a cache, and
queues the sessions that do not yet fit. Every option is optional.

| Option | Argument types | Default | What it sets |
|---|---|---|---|
| [`cap`](#cap) | `const` | `inf` | capacity in units |
| [`block`](#block) | `const` | none | allocation and caching granularity |
| [`evict`](#evict) | `lru` \| `by (expr, …)` | `lru` | which cache entry goes first |
| [`preempt`](#preempt) | `none` \| `lifo` | `none` | what a failed `grow` does |
| [`queue`](#queue) | `fifo` \| `by (expr)` | `fifo` | admission order |
| [`admit via`](#admit-via) | `stage` | none | the queue is served by a step stage |
| [`spill`](#spill) | `pool`, `stage`, `expr`, `expr` | none | evicted prefixes are written to a tier |

The invariant `allocated + cached ≤ cap` holds in every reachable
configuration. A request that can never fit — its units, or its `reserve`
when that is larger, above the cap — is rejected and its session ends. vLLM
judges `max_model_len` instead: it refuses a longer prompt before
scheduling it, and refuses to start a KV cache smaller than one request of
that length, which is what makes every request it admits fit.

## `cap`

```serq
cap expr;
```

Capacity in units. Cached prefixes never block an admission: they are evicted
to make room.

## `block`

```serq
block expr;
```

Allocations round up, and cache entries are kept and evicted, in blocks of this
many units (vLLM's `block_size`). An entry is evicted from its tail, block by
block.

## `evict`

```serq
evict lru;
evict by (k1, k2, …);
```

| Form | Order |
|---|---|
| `lru` | release time, then release order |
| `by (k1, …)` | ascending keys, then release order |

Keys are evaluated per cache entry at the `Evict` moment and may read `size`,
`age`, `last` and `waiting` ([context variables](context.md)), and the entry's
session's attributes: current while the session lives, as they were at its
last release after it has ended. The smallest key goes first.

## `preempt`

```serq
preempt none;
preempt lifo;
```

What a [`grow`](statements.md#grow) (or a `growing` run) does when the pool has no room.

| Form | Behaviour |
|---|---|
| `none` | The session waits and resumes where it was. |
| `lifo` | The most recently admitted holder that is a resident of the step stage this pool is the memory of is preempted (vLLM's `running[-1]`). Its job leaves the stage, its hold is released with the computed prefix cached, and it re-enters the head of the queue to execute its hold again with `computed` set. The grower can be its own victim. |

## `queue`

```serq
queue fifo;
queue by (key, …);
```

`by` reevaluates keys for every waiting hold before each admission attempt,
compares them lexicographically, and breaks ties by enqueue order. One key is
a list of one. `waited` supplies seconds since the current hold entered the
queue; it resets on re-entry. Keys may read visible session attributes,
`now` and pool/stage queries, and may not draw.

A preempted hold re-enters at the head, ahead of policy keys. Only the selected
hold is tried: failure to fit blocks the rest. FIFO keeps enqueue order.
Selection occurs at admission attempts; there is no implicit aging timer.
For a stage-bound pool, `budget_left(stage)` reads the budget remaining for
that admission, and the policy is evaluated again after every admission.

For Ascend-style FCFS lanes with a three-second aging threshold:

```serq
queue by (immediate ? 0 : prompt > 128 && waited >= 3 ? 1 : prompt <= 128 ? 2 : 3,
          serial);
```

This admits immediate requests, then aged long requests, then short requests,
then other long requests. `serial` orders requests within a lane in this
single-hold workload. [Waiting selection](../design/waiting-selection.md)
records the exact scope and regression cases.

## `admit via`

```serq
admit via STAGE;
```

| Argument | Type | Description |
|---|---|---|
| `STAGE` | `stage` (a `step` stage) | Serves this pool's queue at the start of each iteration, after the residents have taken their tokens, while budget is left, and not in an iteration that preempted (vLLM's waiting loop). |

Without it the pool is admitted at every settle. `budget_left(STAGE)` is
meaningful in the hold's header. Arrays join member for member (`pool q[N]`
with `stage S[N]`), a family of one is shared by every member, and any other
pair of counts is a link error. A stage that serves several pools tries them in
declaration order.

With `serve exclusive prefill`, selected prefills stop further admission;
a fitting waiting prefill can displace tentative resident decodes. Its
header sees the full token budget, since the cancelled decodes consume none.
The usual fit, budget-exhaustion and preemption gates still apply.

## `spill`

```serq
spill TIER via LINK (work) when (pred);
```

| Argument | Type | Moment | Description |
|---|---|---|---|
| `TIER` | `pool` | | Where the evicted prefix is written. |
| `LINK` | `stage` | | The stage the write takes time on. |
| `work` | `expr` | `Evict` | Work the write puts on `LINK`. |
| `pred` | `expr` | `Evict` | The prefix is spilled when this is non-zero. May read `size`. |

## In the IR

`CPool { name, cap, block, evict, preempt, queue, spill, admit_via }`; see
[the IR](../ir.md).
