# 2. Memory is a resource

In chapter 1 a request waited for a *server*. In an LLM system it mostly waits
for **memory**: the KV cache is finite, and a request that has nowhere to put
its keys and values cannot start, however idle the GPU is.

seQ has one abstraction for this — a **pool** — and it covers KV bytes, request
slots (`max_num_seqs`), a cap on live sessions and offload tiers alike. A pool
is a counted resource with a capacity, a queue and, later, a cache.

## The program

```seq title="docs/tutorial/programs/02-memory.seq"
--8<-- "docs/tutorial/programs/02-memory.seq"
```

Four servers now, so compute is not the constraint. Ten memory units are.

## `hold` is the whole idea

```seq
hold mem (c) {
  observe admit_wait = now - t0;
  run server (s);
}
```

`hold` is a **scope**. Entering it means: join the pool's queue, wait until
`c` units are free, take them. Leaving it means: give them back. The lecture's
language had `admit` and `free` as separate statements; making them one scoped
statement is the single most important design decision in seQ, for three
reasons:

- **Balance is syntactic.** You cannot forget to free.
- **The memory invariant becomes a lemma about one statement.**
  `allocated + cached ≤ cap` holds in every reachable configuration, and that
  is a theorem in Lean (`SeqLang.Step.invariant`).
- **Preemption is "abort the scope and re-run the statement"** — which,
  it turns out, is exactly what vLLM's `_preempt_request` does
  ([chapter 5](05-the-engine.md)).

!!! info "When are the units counted?"
    `c` is evaluated **when the session is admitted**, not when it joins the
    queue. This matters as soon as the expression reads something that changes
    while you wait — the cache, or the engine's remaining budget. Chapter 5
    depends on it entirely.

## Running it

```bash
seq-lang run docs/tutorial/programs/02-memory.seq
```

```text
run: horizon 100000 warmup 5000 seed 1 events 158920 arrivals 79460 ended 75462 turns 75462 mean live 0.871
observe        count        mean      95% CI      cv2       p99
  admit_wait    75462      0.0947 ±0.0050     13.756    1.8649
  response      75462      1.0959 ±0.0090      0.944    4.8777
stage          number   util    done   thru      wait   service  iters
  server         0.795  0.560   75462  0.7943    0.0000    1.0012      0
pool             used     cached  queue holders    wait  admits evict(n)  evict(u) preempt spill rej
  mem                2.8       0.0  0.075   0.795  0.0947   79460        0         0       0     0   0
```

Note `wait 0.0000` at the stage: nobody queues for a server. All the waiting
has moved to the pool.

## The sweep

```bash
for C in 4 6 10 20; do
  seq-lang run docs/tutorial/programs/02-memory.seq --set C=$C
done
```

| `cap` | admit wait | response | rejected |
|---|---|---|---|
| 4 | 1.1893 | 2.1902 | 19 925 |
| 6 | 1.5358 | 2.5369 | 0 |
| 10 | 0.0947 | 1.0959 | 0 |
| 20 | 0.0006 | 1.0039 | 0 |

Two things are worth stopping on.

**`cap 4` waits *less* than `cap 6`.** It is not better — it is refusing work.
A request drawing `c = 5` can never fit in a pool of 4, so it is rejected
outright and never waits. 19 925 of them. seQ counts that in the `rej` column,
and vLLM does the same thing under the name `FINISHED_IGNORED`.

**The knee is sharp.** Between `cap 6` and `cap 10` the wait falls by a factor
of 16. Capacity planning for memory is not a smooth trade-off; you are either
comfortably above the knee or you are in trouble. Every later chapter sharpens
this.

## Head-of-line blocking

A pool's queue is FIFO by default, and **only the head can be admitted**. If
the request at the front needs 5 units and 3 are free, the request behind it
needing 2 units waits anyway. That is not an accident of the implementation —
it is vLLM's behaviour (`if new_blocks is None: break`), and it is why a
long-context request can stall a queue of short ones.

`queue by (expr)` changes the order if you want to model a priority scheduler
instead.

---

Next: real sessions come back. → [Sessions and turns](03-sessions.md)
