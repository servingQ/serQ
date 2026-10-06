# 1. A queue

Model requests arriving at one server, then compare their simulated waiting
and response times with the M/M/1 queue's closed forms.

## The program

```serq title="docs/tutorial/programs/01-queue.sq"
--8<-- "docs/tutorial/programs/01-queue.sq"
```

This program has four blocks:

### `stage`

```serq
stage server : fifo;
```

A **stage** is where time passes. `fifo` serves one job at a time in arrival
order; `fifo(c)` gives it `c` servers. The other kinds are `ps` (processor
sharing — everyone at once, sharing the throughput), `delay` (everyone at once,
no waiting at all) and `step`, the LLM engine, which arrives in
[chapter 5](05-the-engine.md).

### `workload`

```serq
workload {
  arrive poisson(Lambda);
  turn { set s = ~exp(S); }
}
```

`arrive` says how sessions show up. `turn` is the block that draws the next
turn's attributes; `~exp(S)` is a fresh draw from an exponential with mean `S`.
The other distributions are `~det`, `~uniform`, `~erlang`, `~h2` and
`~bernoulli`.

### `session`

```serq
session {
  turn;
  set t0 = now;
  run server (s);
  observe response = now - t0;
  observe wait = now - t0 - s;
  end;
}
```

A `session` block is what one session does, from arrival to `end`.
`run server (s)` is `s` seconds of work at `server`. `now` is the clock. `observe name = expr` records
a sample — this is how the program says what it measures, rather than the
interpreter guessing.

### `run`

```serq
run { horizon 100000; warmup 5000; seed 1; }
```

How long to simulate, how much to throw away first, and the seed.

## Running it

```bash
serq run docs/tutorial/programs/01-queue.sq
```

```text
run: horizon 100000 end 100000 warmup 5000 seed 1 events 158921 arrivals 79460 ended 75464 turns 75462 mean live 3.768

observe   count    mean   95% CI    cv2      p99
--------  -----  ------  -------  -----  -------
response  75464  4.7432  ±0.2838  1.028  22.9285
wait      75464  3.7458  ±0.2833  1.578  21.7495

stage   number   util   done    thru    wait  service  iters
------  ------  -----  -----  ------  ------  -------  -----
server   3.768  0.792  75464  0.7944  3.7458   0.9974      0
```

## Checking it against the textbook

This is M/M/1 with \(\lambda = 0.8\) and \(\mathbb{E}[S] = 1\), so
\(\rho = 0.8\), and the closed forms are

\[
  \mathbb{E}[R] = \frac{\mathbb{E}[S]}{1-\rho} = 5, \qquad
  \mathbb{E}[W] = \frac{\rho\,\mathbb{E}[S]}{1-\rho} = 4, \qquad
  \mathbb{E}[L] = \frac{\rho}{1-\rho} = 4.
\]

| | closed form | run | |
|---|---|---|---|
| response | 5 | 4.7432 ±0.2838 | covered |
| wait | 4 | 3.7458 ±0.2833 | covered |
| number in system | 4 | 3.768 | |
| utilisation | 0.8 | 0.792 | |

!!! tip "Compare intervals across runs"
    This run's response-time interval includes the theoretical mean of 5.
    A 95% confidence interval can miss the true mean even for a correct
    model. If discrepancies persist across seeds and longer runs, check the
    model assumptions, warm-up and implementation.

## What to try

`Lambda` is declared with `args.number`, so the whole stability curve is one loop:

```bash
for L in 0.5 0.8 0.9 0.95 0.99; do
  serq run docs/tutorial/programs/01-queue.sq --set Lambda=$L --json
done
```

Watch `response` go as \(1/(1-\rho)\) — the shape that every later chapter is a
variation on.

---

Next: the request needs memory as well as a server. → [Memory is a
resource](02-memory.md)
