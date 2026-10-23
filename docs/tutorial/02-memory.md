# 2. Memory is a resource

A request may need both a server and free memory. A **pool** models a
resource with a fixed capacity, such as KV memory or request slots. A
request waits until the pool can supply the units it needs.

## The program

```serq title="docs/tutorial/programs/02-memory.sq"
--8<-- "docs/tutorial/programs/02-memory.sq"
```

```serq title="docs/tutorial/programs/workloads/02-memory.sq"
--8<-- "docs/tutorial/programs/workloads/02-memory.sq"
```

Four servers now, so compute is not the constraint. Ten memory units are.

## Reserve memory with `hold`

```serq
Cost processing = { svc: S * work_size, mem: items };
hold mem (processing.mem) {
  observe admit_wait = now - t0;
  run svc (processing.svc);
}
```

The workload draws `work_size` and `items` and sends both, `send
(work_size, items)`, to `server (work_size, items)`. The server converts
them to service work and memory demand; here each item needs one memory
unit.

`hold` joins the pool's queue, waits for `processing.mem` units and reserves them for the
body. When the body finishes, the units are released automatically.

!!! info "When are the units counted?"
    The declaration computes `processing.mem` once. The hold reads that
    stored amount **at admission**. A header that reads changing cache or
    budget state must calculate it at admission instead; Chapter 5 writes
    those expressions in the engine's `taking`, which is read at admission.

## Running it

```bash
serq run docs/tutorial/programs/02-memory.sq --workload docs/tutorial/programs/workloads/02-memory.sq --horizon 100000 --warmup 5000 --seed 1
```

```text
run: horizon 100000 end 100000 warmup 5000 seed 1 events 158921 arrivals 79460 ended 75462 turns 75462 mean live 0.870

observe     count    mean   95% CI     cv2     p99
----------  -----  ------  -------  ------  ------
admit_wait  75462  0.0961  ±0.0056  13.711  1.8797

client    count   mean  95% CI    cv2    p99
--------  -----  -----  ------  -----  -----
response  75462  1.096  ±0.014  0.943  4.886

stage  number   util   done    thru    wait  service  iters
-----  ------  -----  -----  ------  ------  -------  -----
svc     0.794  0.559  75462  0.7943  0.0000   0.9997      0

pool  used  cached  queue  holders    wait  admits  evict(n)  evict(u)  preempt  spill  rej  stuck
----  ----  ------  -----  -------  ------  ------  --------  --------  -------  -----  ---  -----
mem    2.8     0.0  0.076    0.794  0.0961   79460         0         0        0      0    0      0
```

Note `wait 0.0000` at the stage: nobody queues for a server. All the waiting
has moved to the pool.

## The sweep

```bash
for C in 4 6 10 20; do
  serq run docs/tutorial/programs/02-memory.sq --workload docs/tutorial/programs/workloads/02-memory.sq --horizon 100000 --warmup 5000 --seed 1 --C $C
done
```

| `cap` | admit wait | response | rejected |
|---|---|---|---|
| 4 | 1.2384 | 2.2379 | 19,803 |
| 6 | 1.5808 | 2.5805 | 0 |
| 10 | 0.0961 | 1.0958 | 0 |
| 20 | 0.0006 | 1.0026 | 0 |

At capacity 4, a request needing 5 units is rejected before it waits.
The 19,803 rejected sessions explain why mean waiting time can look
better than at capacity 6: the means describe different admitted workloads.
Compare rejection counts as well as latency.

For this workload, increasing capacity from 6 to 10 sharply reduces
admission wait; increasing it to 20 leaves little waiting at either resource.

## Head-of-line blocking

A pool's queue is FIFO by default, and **only the head can be admitted**. If
the request at the front needs 5 units and 3 are free, the request behind it
needing 2 units waits anyway. A large request can therefore delay smaller requests behind it.
vLLM's scheduler does the same: its waiting loop stops at the first request
whose blocks it cannot allocate (`if new_blocks is None: break`,
`scheduler.py:1228-1235`).

`queue by (expr)` changes the order if you want to model a priority scheduler
instead. `first fit` changes what happens to a request that does not fit: it
is passed over, and the first one behind it that fits is admitted.
`docs/tutorial/programs/02-first-fit.sq` is this chapter's program with that
one line changed:

```serq
--8<-- "docs/tutorial/programs/02-first-fit.sq:pool"
```

```bash
for p in 02-memory 02-first-fit; do
  serq run docs/tutorial/programs/$p.sq --workload docs/tutorial/programs/workloads/02-memory.sq --horizon 100000 --warmup 5000 --seed 1 --C 6
done
```

| `cap 6` | admit wait | its p99 | response |
|---|---|---|---|
| `02-memory.sq`: FIFO, the head blocks | 1.5808 | 10.9918 | 2.5805 |
| `02-first-fit.sq`: `queue fifo first fit;` | 1.0687 | 8.7344 | 2.0684 |

The small requests no longer wait behind a large one, and the queue drains
faster. Nothing in the rule bounds how long a large request waits, though:
each attempt starts again from the head, and a request needing 5 units is
passed over for as long as smaller ones keep fitting. Head-of-line
blocking is what keeps the room for it.

---

Next: real sessions come back. → [Sessions and turns](03-sessions.md)
