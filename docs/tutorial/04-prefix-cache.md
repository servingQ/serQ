# 4. The prefix cache

Reuse a session's cached KV between turns to avoid recomputing its context.
The prefix can be evicted while the session thinks or waits for admission.

## The program

```serq title="docs/tutorial/programs/04-cache.sq"
--8<-- "docs/tutorial/programs/04-cache.sq"
```

```serq title="docs/tutorial/programs/workloads/04-cache.sq"
--8<-- "docs/tutorial/programs/workloads/04-cache.sq"
```

## `cache`, `cached`, `evict`, `drop`

```serq
hold kv (cost(kv, K + n + o)) {
  set warm = cached >= K;
  run engine (cost(engine, (warm ? a * n : a * (K + n)) + d * o));
} cache kv (cost(kv, K + n + o));
```

**`cache kv (ℓ)`** at the end of a hold: when the scope ends the units are
released, but up to `ℓ` of them stay in `kv` as this session's cached
prefix. Cached units do not block anybody — a request that needs room evicts
them — but they occupy the pool. The clause is also
what makes the hold *take* from the cache: a hold without it leaves this
session's cached prefix where it is (`cached` is 0 in its body), and
`cache kv (0)` consumes the prefix and keeps nothing. It names its pool
because a hold may take several, and only one holds the prefix: a hold of
a request slot and the KV (`hold reqs (…), kv (…)`) caches `kv`, and the
slot is memory alone.

**`cached`** is how much of the session's own prefix survived, read at the
moment the hold is **admitted**. Not when it queued: while it waits, its
prefix is evictable. That gap is the *wait channel*, and chapter 6 is about
what it does.

**`evict lru`** orders eviction by release time. `evict by (k₁, …)` orders by
whatever the program says — `replica.sq` uses `evict by (waiting, size)`, which
evicts non-queued sessions' prefixes before queued sessions' prefixes,
shortest first within each group.

**`drop kv;`** discards the session's prefix. Without it, cached units can
remain after the session ends, until they are evicted. The KV is the
deployment's, so the server drops it: the workload says which turn is the
session's last, `send (K, n, o, final: more == 0);`, drawing `more` with
the turn's sizes, and the server ends with

```serq
if (final) { drop kv; }
```

## Running it

```bash
serq run docs/tutorial/programs/04-cache.sq --workload docs/tutorial/programs/workloads/04-cache.sq --horizon 20000 --warmup 2000 --seed 1
```

```text
run: horizon 20000 end 20000 warmup 2000 seed 1 events 99813 arrivals 9905 ended 8902 turns 44889 mean live 6.143

observe  count    mean   95% CI    cv2     p99
-------  -----  ------  -------  -----  ------
hit      35986  1.0000  ±0.0000  0.000  1.0000

client    count     mean    95% CI    cv2      p99
--------  -----  -------  --------  -----  -------
response  44889  0.06336  ±0.00074  0.640  0.24050

stage   number   util   done    thru    wait  service  iters
------  ------  -----  -----  ------  ------  -------  -----
engine   0.158  0.140  44889  2.4938  0.0074   0.0560      0
tool     5.985  0.998  35986  1.9992  0.0000   2.9932      0

pool   used   cached  queue  holders    wait  admits  evict(n)  evict(u)  preempt  spill  rej  stuck
----  -----  -------  -----  -------  ------  ------  --------  --------  -------  -----  ---  -----
kv    796.3  30195.3  0.000    0.158  0.0000   49906         0         0        0      0    0      0
```

Every turn after a session's first is a hit, and the response time falls
from 0.178 s to 0.063 s against chapter 3 — nearly a factor of three, from
one clause. The pool is holding 30 195 tokens of cache against a capacity
of 200 000, so nothing is ever evicted.

The workload rounds what it draws, `ceil(…)` for the new tokens and
`floor(…) + 1` for the output: a token count is a whole number from where a
KV pool reads it, and serQ refuses a program in which one may be
fractional there. Chapter 3 has no KV pool, so its workload keeps its real
draws; rounding raises each draw's mean by half a token (`ceil` of an
exponential of mean 500 has mean 500.5), so the comparison with chapter 3
is between two workloads that differ by that half token (against real
draws, the rounding adds 0.1 ms of service and 0.16 ms of response per
turn here). With real-valued counts this run read a
hit rate of 0.9999 with nothing evicted: `K` and `cached` were sums of
fractions, and in five turns the cached prefix read short of `K` by a
rounding error ([#661](https://github.com/servingQ/serQ/issues/661); the
rule is in [Token counts](../language.md#token-counts)).

## Vary cache capacity

```bash
for C in 2e5 6e4 4e4 3e4 2e4 1.5e4 1e4; do
  serq run docs/tutorial/programs/04-cache.sq --workload docs/tutorial/programs/workloads/04-cache.sq --horizon 20000 --warmup 2000 --seed 1 --C $C --json
done
```

| `cap` | hit rate | response (s) | entries evicted | rejected |
|---|---|---|---|---|
| 200,000 | 1.000 | 0.0634 | 0 | 0 |
| 60,000 | 0.972 | 0.0692 | 1,112 | 0 |
| 40,000 | 0.850 | 0.0885 | 6,056 | 0 |
| 30,000 | 0.690 | 0.1101 | 12,537 | 4 |
| 20,000 | 0.463 | 0.1365 | 21,815 | 47 |
| 15,000 | 0.355 | 0.1403 | 25,968 | 214 |
| 10,000 | 0.259 | 0.1312 | 28,208 | 856 |

Measured 2026-10-15 on serQ afee627 with whole token counts (#661, #700), by the
commands above.

At 10 000 units the mean response time is lower than at 15 000 despite a
lower hit rate, but rejections rise from 214 to 856. Requests that cannot
fit do not contribute completed response samples, so the lower mean alone
does not establish better performance.

The default run caches 30 195 tokens on average. Most live sessions are in
the tool stage, so the cache stores context that the engine is not currently
using. Reducing its capacity forces more of that context to be recomputed.

---

Next: prefill and decode share one iteration. → [The engine](05-the-engine.md)
