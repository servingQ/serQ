# Pool

```serq
pool NAME [ '[' N ']' ] {
  cap expr;
  block expr;
  evict lru;  |  evict by (expr, …);
  preempt none;  |  preempt lifo;  |  preempt by (expr, …) [requeue head | tail];
  queue fifo;  |  queue by (key, …);
  reserve held;
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
| [`preempt`](#preempt) | `none` \| `by (expr, …) [requeue head \| tail]` \| `lifo` | `none` | what a failed `grow` does: whom it preempts, and where the victim goes back |
| [`queue`](#queue) | `fifo` \| `by (expr, …)` | `fifo` | admission order |
| [`admit via`](#admit-via) | `stage` | none | the queue is served by a step stage |
| [`reserve held`](#reserve-held) | — | off | a hold's unallocated `reserve` counts against later admissions |
| [`spill`](#spill) | `pool`, `stage`, `expr`, `expr` | none | evicted prefixes are written to a tier |

If either `units` or `reserve` is independent of deployment state and
exceeds capacity after block rounding, the hold is rejected and its session ends. State-dependent demand waits for admission and is reported as
[`over_cap`](../reference/cli.md#pool-statistics) if it still exceeds capacity
at the end of the run.

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
many units. An entry is evicted from its tail, block by
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
preempt by (key, …) [requeue head | requeue tail];
preempt lifo;
```

What a [`grow`](statements.md#grow) (or a `growing` run) does when the pool has no room.

| Form | Behaviour |
|---|---|
| `none` | Wait for room, then resume. |
| `by (k, …)` | Preempt the candidate with the least keys, compared in order; ties go to the last admitted. |
| `lifo` | Shorthand for `by (-admission) requeue head`: preempt the last admitted. |

Candidates are holders resident in the stage whose `memory` is this pool.
If no stage names it as memory, candidates are holders with an active hold
scope. Leases are not candidates; the grower can be its own victim.

The victim leaves its stage, releases its hold and caches its computed prefix.
It re-enters the hold's first pool's queue to execute the hold again with
`computed` set to the position reached. `requeue head` (the default) places
it first, ahead of policy keys. `requeue tail` treats it as a newcomer:
FIFO places it last, while `queue by` places it by its keys, with `waited`
reset to 0.

Keys are read at the `Victim` moment, for each candidate: its visible
attributes, `admission` (its place in the candidates' admission order: the
engine's serving order, or the order the pool admitted its holders),
`decoding` (1 for a decoding resident) and `position` (the position its hold
has computed on the pool, [context variables](context.md)), constants, `now`
and pool and stage queries. A key may not draw, read `budget_left(…)`, or
read `computed`, which is the position at the session's last preemption,
not where the candidate is now.

For a priority queue where larger priority values lose first:

```serq
queue by (priority, t0);
preempt by (-priority, -t0) requeue tail;
```

Here `priority` and arrival time `t0` are attributes set by the program.

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

A preempted hold re-enters at the head, ahead of policy keys, unless its pool
says `requeue tail`. Only the selected
hold is tried: failure to fit blocks the rest. FIFO keeps enqueue order.
Selection occurs at admission attempts; there is no implicit aging timer.
For a stage-bound pool, `budget_left(stage)` reads the budget remaining for
that admission, and the policy is evaluated again after every admission.

For priority lanes with a three-second aging threshold:

```serq
queue by (immediate ? 0 : prompt > 128 && waited >= 3 ? 1 : prompt <= 128 ? 2 : 3,
          serial);
```

This admits immediate requests, then aged long requests, then short requests,
then other long requests. `serial` orders requests within a lane in this
single-hold workload.

## `reserve held`

```serq
reserve held;
```

By default, `reserve` is checked only at admission. With `reserve held`,
each live hold's unallocated reservation also counts against later
admissions and growth by other holds:

$$
u_i = \max(0, r_i-a_i),
$$

where $a_i$ is allocated units and $r_i$ is the larger of `units` and
`reserve`, evaluated at admission and rounded up to blocks. A new reservation $r$ requires
$\mathrm{used}+\sum_i u_i+r\leq\mathrm{cap}$. Growth by $d$ requires
$\mathrm{used}+\sum_{i\ne\mathrm{grower}}u_i+d\leq\mathrm{cap}$: the growing
hold can use its own reservation.

Each hold counts separately, including nested holds. Reservations end with
the hold's scope, `release` or preemption; leases retain none. Cached
prefixes are evicted when units are allocated, not when reserved. A hold
that grows beyond its reservation follows the pool's `preempt` rule.

## `admit via`

```serq
admit via STAGE;
```

| Argument | Type | Description |
|---|---|---|
| `STAGE` | `stage` (a `step` stage) | Serves this pool's queue at the start of each iteration, after the residents have taken their tokens, while budget is left, and not in an iteration that preempted under the default iteration procedure. |

Without it, admission is attempted whenever the simulation settles an event. `budget_left(STAGE)` is
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

## Examples

A complete program:

```serq
fn main() {
  pool reqs { cap 2; }
  stage svc : delay;
  workload {
    arrive batch(3);
  }
  server {
    set t0 = now;
    hold reqs (1) { run svc (2); }
    observe response = now - t0;
  }
}
```

Save as `model.sq`, then run:

```sh
serq run model.sq --horizon 10
```

## See also

[`hold`](statements.md#hold), [pool queries](functions.md#pool),
[`pyserq.Pool`](../python/pool.md).
