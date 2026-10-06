# 4. The prefix cache

Reuse a session's cached KV between turns to avoid recomputing its context.
The prefix can be evicted while the session thinks or waits for admission.

## The program

```serq title="docs/tutorial/programs/04-cache.sq"
--8<-- "docs/tutorial/programs/04-cache.sq"
```

## `cache`, `cached`, `evict`, `drop`

```serq
hold kv (K + n + o) {
  set hit = cached >= K;
  run engine ((hit ? a * n : a * (K + n)) + d * o);
} cache (K + n + o);
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
remain after the session ends, until they are evicted.

## Running it

```bash
serq run docs/tutorial/programs/04-cache.sq --horizon 20000 --warmup 2000 --seed 1
```

```text
run: horizon 20000 end 20000 warmup 2000 seed 1 events 98781 arrivals 9905 ended 8905 turns 44301 mean live 6.006

observe   count    mean   95% CI    cv2     p99
--------  -----  ------  -------  -----  ------
response  44301  0.0629  ±0.0006  0.615  0.2317
hit       35398  1.0000  ±0.0000  0.000  1.0000

stage   number   util   done    thru    wait  service  iters
------  ------  -----  -----  ------  ------  -------  -----
engine   0.155  0.138  44301  2.4612  0.0070   0.0559      0
tool     5.851  0.997  35398  1.9666  0.0000   2.9755      0

pool   used   cached  queue  holders    wait  admits  evict(n)  evict(u)  preempt  spill  rej  stuck
----  -----  -------  -----  -------  ------  ------  --------  --------  -------  -----  ---  -----
kv    763.5  28966.3  0.000    0.155  0.0000   49390         0         0        0      0    0      0
```

A perfect hit rate, and the response time falls from 0.183 s to 0.063 s against
chapter 3 — nearly a factor of three, from one clause. The pool is holding
28 966 tokens of cache against a capacity of 200 000, so nothing is ever
evicted.

## Vary cache capacity

```bash
for C in 2e5 6e4 4e4 3e4 2e4 1.5e4 1e4; do
  serq run docs/tutorial/programs/04-cache.sq --horizon 20000 --warmup 2000 --seed 1 --set C=$C --json
done
```

| `cap` | hit rate | response (s) | entries evicted | rejected |
|---|---|---|---|---|
| 200,000 | 1.000 | 0.0629 | 0 | 0 |
| 60,000 | 0.980 | 0.0664 | 827 | 0 |
| 40,000 | 0.854 | 0.0856 | 5,848 | 0 |
| 30,000 | 0.699 | 0.1083 | 12,046 | 1 |
| 20,000 | 0.472 | 0.1354 | 21,208 | 35 |
| 15,000 | 0.346 | 0.1444 | 26,163 | 174 |
| 10,000 | 0.256 | 0.1313 | 28,225 | 846 |

At 10 000 units the mean response time is lower than at 15 000 despite a
lower hit rate, but rejections rise from 174 to 846. Requests that cannot
fit do not contribute completed response samples, so the lower mean alone
does not establish better performance.

The default run caches 28 966 tokens on average. Most live sessions are in
the tool stage, so the cache stores context that the engine is not currently
using. Reducing its capacity forces more of that context to be recomputed.

---

Next: prefill and decode share one iteration. → [The engine](05-the-engine.md)
