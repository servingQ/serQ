# 3. Sessions and turns

An agent does not send one request. It sends a prompt, reads the answer, calls
a tool, thinks for a few seconds, and comes back with a longer context. The
system sees a *session* with many **turns**, and the gaps between turns are
where all the interesting behaviour lives.

## The program

```seq title="docs/tutorial/programs/03-sessions.seq"
--8<-- "docs/tutorial/programs/03-sessions.seq"
```

## What is new

### `loop` and `turn`

```seq
session {
  turn;
  loop {
    …
    branch with (p) { run tool (~exp(Z)); turn; } else { end; }
  }
}
```

`turn;` runs the workload's `turn` block, drawing the next turn's attributes.
`loop` repeats until something inside it ends the session.

There are two branches, and the difference matters. `branch (e) { … } else { … }`
is the **conditional** — it takes the first block when `e` is non-zero, so the
guard can depend on the session's state. `branch with (p) { … }` is a **draw**:
it takes the first block with probability `p`. Here the session continues with
probability `p = 0.8`, so it is a draw, and writing it that way is what lets a
reader — and the figure — tell it from a test.

### Attributes carry state across turns

```seq
init { set K = 0; }
…
set K = K + n + o;
```

`init` runs once when the session arrives; `set` assigns a **session
attribute**, which survives from turn to turn. `K` is the context the session
has accumulated. This is the variable everything else in the tutorial depends
on: it grows without bound, and memory does not.

### `delay`

```seq
stage tool : delay;
```

A `delay` stage is an infinite-server station: every job runs immediately at
rate 1, nobody waits. It is how you model something outside your system — a
tool call, a human reading, a downstream API.

## Running it

```bash
seq-lang run docs/tutorial/programs/03-sessions.seq
```

```text
run: horizon 20000 warmup 2000 seed 1 events 97621 arrivals 9905 ended 8903 turns 43950 mean live 6.262
observe        count        mean      95% CI      cv2       p99
  response      43950      0.1801 ±0.0069      0.467    0.6162
  context       43950   4935.0401 ±118.4264    0.436 16699.0270
stage          number   util    done   thru      wait   service  iters
  engine         0.440  0.329   43950  2.4417    0.0453    0.1348      0
  tool           5.822  0.998   35047  1.9471    0.0000    2.9902      0
```

Three numbers are worth reading carefully.

**9 905 arrivals, 43 950 turns.** Each session takes about 4.4 turns, and
\(\mathbb{E}[J] = 1/(1-p) = 5\) — the gap is the sessions that were still
running when the horizon ended.

**`mean live 6.262`, of which `tool` holds 5.822.** Most sessions that exist
are not in your system at all; they are thinking. This is the single biggest
difference between agentic serving and classical queueing, and it is why the
KV cache is interesting: those 5.8 sessions have context you could keep, or
throw away.

**Mean context 4 935 tokens.** It grows every turn. Chapter 4 is about what
happens when you try to keep it.

## What to try

Turn the thinking time down and watch the sessions pile back in:

```bash
for Z in 0.5 1 3 10; do
  seq-lang run docs/tutorial/programs/03-sessions.seq --set Z=$Z
done
```

---

Next: keep the context between turns. → [The prefix cache](04-prefix-cache.md)
