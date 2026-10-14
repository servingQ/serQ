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
} cache (cost(kv, K + n + o));
```

**`cache (ℓ)`** at the end of a hold: when the scope ends the units are
released, but up to `ℓ` of them stay in the pool as this session's cached
prefix. Cached units do not block anybody — a request that needs room evicts
them — but they occupy the pool. The clause is also
what makes the hold *take* from the cache: a hold without it leaves this
session's cached prefix where it is (`cached` is 0 in its body), and
`cache (0)` consumes the prefix and keeps nothing.

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
run: horizon 20000 end 20000 warmup 2000 seed 1 events 99813 arrivals 9905 ended 8902 turns 44889 mean live 6.142

observe  count    mean   95% CI    cv2     p99
-------  -----  ------  -------  -----  ------
hit      35986  0.9999  ±0.0001  0.000  1.0000

client    count     mean    95% CI    cv2      p99
--------  -----  -------  --------  -----  -------
response  44889  0.06320  ±0.00072  0.640  0.23978

stage   number   util   done    thru    wait  service  iters
------  ------  -----  -----  ------  ------  -------  -----
engine   0.158  0.139  44889  2.4938  0.0073   0.0559      0
tool     5.985  0.998  35986  1.9992  0.0000   2.9932      0

pool   used   cached  queue  holders    wait  admits  evict(n)  evict(u)  preempt  spill  rej  stuck
----  -----  -------  -----  -------  ------  ------  --------  --------  -------  -----  ---  -----
kv    793.4  30165.3  0.000    0.158  0.0000   49906         0         0        0      0    0      0
```

A hit rate of 0.9999, and the response time falls
from 0.178 s to 0.063 s against chapter 3 — nearly a factor of three, from
one clause. The pool is holding 30 165 tokens of cache against a capacity
of 200 000, so nothing is ever evicted. The five misses are no evictions:
`K` and `cached` are sums of real-valued token counts, and in five turns
the cached prefix reads short of `K` by a rounding error
([#661](https://github.com/servingQ/serQ/issues/661)).

## Vary cache capacity

```bash
for C in 2e5 6e4 4e4 3e4 2e4 1.5e4 1e4; do
  serq run docs/tutorial/programs/04-cache.sq --workload docs/tutorial/programs/workloads/04-cache.sq --horizon 20000 --warmup 2000 --seed 1 --C $C --json
done
```

| `cap` | hit rate | response (s) | entries evicted | rejected |
|---|---|---|---|---|
| 200,000 | 1.000 | 0.0632 | 0 | 0 |
| 60,000 | 0.972 | 0.0689 | 1,098 | 0 |
| 40,000 | 0.850 | 0.0883 | 6,036 | 0 |
| 30,000 | 0.690 | 0.1101 | 12,535 | 4 |
| 20,000 | 0.463 | 0.1366 | 21,824 | 47 |
| 15,000 | 0.356 | 0.1401 | 25,973 | 213 |
| 10,000 | 0.259 | 0.1309 | 28,196 | 854 |

Measured 2026-10-14 on serQ 1b0a47b with the paired-workload change (the
workload a file of its own), by the commands above.

At 10 000 units the mean response time is lower than at 15 000 despite a
lower hit rate, but rejections rise from 213 to 854. Requests that cannot
fit do not contribute completed response samples, so the lower mean alone
does not establish better performance.

The default run caches 30 165 tokens on average. Most live sessions are in
the tool stage, so the cache stores context that the engine is not currently
using. Reducing its capacity forces more of that context to be recomputed.

---

Next: prefill and decode share one iteration. → [The engine](05-the-engine.md)
