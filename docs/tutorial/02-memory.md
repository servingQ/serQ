# 2. Memory is a resource

A request may need both a server and free memory. A **pool** models a
resource with a fixed capacity, such as KV memory or request slots. A
request waits until the pool can supply the units it needs.

## The program

```serq title="docs/tutorial/programs/02-memory.sq"
--8<-- "docs/tutorial/programs/02-memory.sq"
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

The workload draws `work_size` and `items`. The server converts them to
service work and memory demand; here each item needs one memory unit.

`hold` joins the pool's queue, waits for `processing.mem` units and reserves them for the
body. When the body finishes, the units are released automatically.

!!! info "When are the units counted?"
    The declaration computes `processing.mem` once. The hold reads that
    stored amount **at admission**. A header that reads changing cache or
    budget state must calculate it at admission instead; Chapter 5 uses
    those expressions directly in the header.

## Running it

```bash
serq run docs/tutorial/programs/02-memory.sq --horizon 100000 --warmup 5000 --seed 1
```

```text
run: horizon 100000 end 100000 warmup 5000 seed 1 events 158921 arrivals 79460 ended 75463 turns 75462 mean live 0.866

observe     count    mean   95% CI     cv2     p99
----------  -----  ------  -------  ------  ------
admit_wait  75462  0.0929  ±0.0072  13.811  1.8108
response    75463  1.0903  ±0.0100   0.937  4.8689

stage  number   util   done    thru    wait  service  iters
-----  ------  -----  -----  ------  ------  -------  -----
svc     0.792  0.559  75463  0.7943  0.0000   0.9974      0

pool  used  cached  queue  holders    wait  admits  evict(n)  evict(u)  preempt  spill  rej  stuck
----  ----  ------  -----  -------  ------  ------  --------  --------  -------  -----  ---  -----
mem    2.8     0.0  0.074    0.792  0.0929   79460         0         0        0      0    0      0
```

Note `wait 0.0000` at the stage: nobody queues for a server. All the waiting
has moved to the pool.

## The sweep

```bash
for C in 4 6 10 20; do
  serq run docs/tutorial/programs/02-memory.sq --horizon 100000 --warmup 5000 --seed 1 --set C=$C
done
```

| `cap` | admit wait | response | rejected |
|---|---|---|---|
| 4 | 1.1921 | 2.1893 | 19,957 |
| 6 | 1.5078 | 2.5051 | 0 |
| 10 | 0.0929 | 1.0903 | 0 |
| 20 | 0.0006 | 1.0002 | 0 |

At capacity 4, a request needing 5 units is rejected before it waits.
The 19,957 rejected sessions explain why mean waiting time can look
better than at capacity 6: the means describe different admitted workloads.
Compare rejection counts as well as latency.

For this workload, increasing capacity from 6 to 10 sharply reduces
admission wait; increasing it to 20 leaves little waiting at either resource.

## Head-of-line blocking

A pool's queue is FIFO by default, and **only the head can be admitted**. If
the request at the front needs 5 units and 3 are free, the request behind it
needing 2 units waits anyway. A large request can therefore delay smaller requests behind it.

`queue by (expr)` changes the order if you want to model a priority scheduler
instead.

---

Next: real sessions come back. → [Sessions and turns](03-sessions.md)
