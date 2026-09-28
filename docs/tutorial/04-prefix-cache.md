# 4. The prefix cache

Chapter 3 recomputed every turn's whole context. Nobody does that: the KV of
turn \(j\) is still in memory when turn \(j+1\) arrives, unless something threw
it away. Whether it is still there is the single most important thing about an
agentic serving system, and it is not a property of one request — it is a
property of the traffic.

## The program

```rust title="docs/tutorial/programs/04-cache.seq"
--8<-- "docs/tutorial/programs/04-cache.seq"
```

## `cache`, `cached`, `evict`, `drop`

```rust
hold kv (K + n + o) {
  set hit = cached >= K;
  run engine ((hit ? a * n : a * (K + n)) + d * o);
} cache (K + n + o);
```

**`cache (ℓ)`** at the end of a hold: when the scope ends the units are
released, but up to `ℓ` of them stay in the pool as this session's cached
prefix. Cached units do not block anybody — a request that needs room evicts
them — but they occupy the pool, and the invariant
`allocated + cached ≤ cap` holds in every reachable state.

**`cached`** is how much of the session's own prefix survived, read at the
moment the hold is **admitted**. Not when it queued: while it waits, its
prefix is evictable. That gap is the *wait channel*, and chapter 6 is about
what it does.

**`evict lru`** orders eviction by release time. `evict by (k₁, …)` orders by
whatever the program says — `agentic.seq` uses `evict by (queued, size)`, which
throws out queued sessions' short prefixes first.

**`drop kv;`** before `end` discards the session's prefix. Without it the
prefix survives the session, which is not an oversight: vLLM keeps a finished
request's blocks in the free queue, and `programs/vllm.seq` models that by not
dropping.

## Running it

```bash
seq-lang run docs/tutorial/programs/04-cache.seq
```

```text
run: horizon 20000 warmup 2000 seed 1 events 97621 arrivals 9905 ended 8903 turns 43947 mean live 5.977
observe        count        mean      95% CI      cv2       p99
  response      43947      0.0635 ±0.0007      0.626    0.2375
  hitrate       35044      1.0000 ±0.0000      0.000    1.0000
stage          number   util    done   thru      wait   service  iters
  engine         0.155  0.137   43947  2.4415    0.0073    0.0562      0
  tool           5.822  0.998   35044  1.9469    0.0000    2.9903      0
pool             used     cached  queue holders    wait  admits evict(n)  evict(u) preempt spill rej
  kv               758.5   28834.6  0.000   0.155  0.0000   48810        0         0       0     0   0
```

A perfect hit rate, and the response time falls from 0.180 s to 0.064 s against
chapter 3 — nearly a factor of three, from one clause. The pool is holding
28 835 tokens of cache against a capacity of 200 000, so nothing is ever
evicted.

## The sweep that matters

```bash
for C in 2e5 6e4 4e4 3e4 2e4 1.5e4 1e4; do
  seq-lang run docs/tutorial/programs/04-cache.seq --set C=$C --json
done
```

| `cap` | hit rate | response | entries evicted | mean cached |
|---|---|---|---|---|
| 200 000 | 1.000 | 0.0635 | 0 | 28 835 |
| 60 000 | 0.979 | 0.0672 | 789 | 28 107 |
| 40 000 | 0.868 | 0.0848 | 5 220 | 23 990 |
| 30 000 | 0.719 | 0.1049 | 11 130 | 19 094 |
| 20 000 | 0.489 | 0.1296 | 20 196 | 12 055 |
| 15 000 | 0.356 | 0.1383 | 25 243 | 8 215 |
| 10 000 | 0.260 | **0.1307** | 27 696 | 4 949 |

Read the last row twice. Going from 15 000 to 10 000 units the hit rate keeps
falling, but the **response time improves**. That is not an error. At 10 000
the only prefixes that survive are short ones, so the sessions that hit are
cheap and the ones that miss were going to be expensive anyway; the mean moves
for a reason that has nothing to do with the system getting better.

This is why `observe` exists and why the report gives you the pool counters
next to the times. A hit rate is not a performance number, and a mean is not a
system.

## The pool is holding 28 835 tokens. Where does that come from?

`mean live 5.977`, of which 5.822 are in the tool call (chapter 3). Those
5.8 thinking sessions each have a context in the pool that nobody is using and
everybody is paying for. The KV cache of an agentic workload is mostly storage
for sessions that are not there.

Squeeze it and they lose their prefixes. Chapter 6.

---

Next: prefill and decode share one iteration. → [The engine](05-the-engine.md)
