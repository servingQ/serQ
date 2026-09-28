# 1. A queue

Before there are tokens, there is a queue. A serving system is, at bottom, a
thing that makes requests wait — and the whole of queueing theory is about how
long.

## The program

```rust title="docs/tutorial/programs/01-queue.seq"
--8<-- "docs/tutorial/programs/01-queue.seq"
```

Four blocks, and every seQ program has the same four.

### `stage`

```rust
stage server : fifo;
```

A **stage** is where time passes. `fifo` serves one job at a time in arrival
order; `fifo(c)` gives it `c` servers. The other kinds are `ps` (processor
sharing — everyone at once, sharing the throughput), `delay` (everyone at once,
no waiting at all) and `step`, the LLM engine, which arrives in
[chapter 5](05-the-engine.md).

### `workload`

```rust
workload {
  arrive poisson(Lambda);
  turn { set s = ~exp(S); }
}
```

`arrive` says how sessions show up. `turn` is the block that draws the next
turn's attributes; `~exp(S)` is a fresh draw from an exponential with mean `S`.
The other distributions are `~det`, `~uniform`, `~erlang`, `~h2` and
`~bernoulli`.

### `route`

```rust
route {
  turn;
  set t0 = now;
  run server (s);
  observe response = now - t0;
  observe wait = now - t0 - s;
  end;
}
```

The **route** is the program every session runs. `run server (s)` is `s`
seconds of work at `server`. `now` is the clock. `observe name = expr` records
a sample — this is how the program says what it measures, rather than the
interpreter guessing.

### `run`

```rust
run { horizon 100000; warmup 5000; seed 1; }
```

How long to simulate, how much to throw away first, and the seed.

## Running it

```bash
seq-lang run docs/tutorial/programs/01-queue.seq
```

```text
run: horizon 100000 warmup 5000 seed 1 events 158920 arrivals 79460 ended 75464 turns 75462 mean live 3.816
observe        count        mean      95% CI      cv2       p99
  response      75464      4.8040 ±0.2377      0.916   20.4865
  wait          75464      3.8023 ±0.2330      1.394   19.3448
stage          number   util    done   thru      wait   service  iters
  server         3.816  0.796   75464  0.7944    3.8023    1.0017      0
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
| response | 5 | 4.8040 ±0.2377 | covered |
| wait | 4 | 3.8023 ±0.2330 | covered |
| number in system | 4 | 3.816 | |
| utilisation | 0.8 | 0.796 | |

!!! warning "Read the interval, not the mean"
    `4.8040` is not 5, and it is not supposed to be. The interval is what makes
    the claim: `±0.2377` covers 5. A run whose interval does *not* cover the
    closed form is a bug — in the program, or in seQ. That is exactly how
    `programs/mg1.seq`, `ps.seq` and `closed.seq` are checked in CI.

## What to try

`--set` overrides any `let`, so the whole stability curve is one loop:

```bash
for L in 0.5 0.8 0.9 0.95 0.99; do
  seq-lang run docs/tutorial/programs/01-queue.seq --set Lambda=$L --json
done
```

Watch `response` go as \(1/(1-\rho)\) — the shape that every later chapter is a
variation on.

---

Next: the request needs memory as well as a server. → [Memory is a
resource](02-memory.md)
