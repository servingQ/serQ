# 6. The cliff

Everything so far degraded gracefully. Turn the load up on the engine of
chapter 5 and it does not.

## The sweep

```bash
for L in 1.5 1.7 1.8 1.9 2.0; do
  seq-lang run docs/tutorial/programs/05-engine.seq --set Lambda=$L --json
done
```

| sessions/s | hit rate | **TTFT** | preemptions | engine utilisation |
|---|---|---|---|---|
| 1.5 | 0.512 | **0.364 s** | 22 043 | 0.628 |
| 1.7 | 0.490 | **3.058 s** | 99 128 | 0.872 |
| 1.8 | 0.498 | **40.06 s** | 169 930 | 1.000 |
| 1.9 | 0.494 | 85.44 s | 173 225 | 1.000 |
| 2.0 | 0.486 | 168.9 s | 174 634 | 1.000 |

Between 1.7 and 1.8 sessions per second — a 6 % increase in load — the time to
first token goes up by a factor of thirteen. There is no knee to plan against
here; there is an edge.

## Is it just saturation?

The obvious reading is that the engine ran out of compute at \(\rho = 1\). It
did, but that is the symptom. Run the same loads with ten times the KV pool
and nothing else changed:

```bash
seq-lang run docs/tutorial/programs/05-engine.seq --set Lambda=2.0 --set blocks=40000
seq-lang run docs/tutorial/programs/05-engine.seq --set Lambda=3.0 --set blocks=40000
```

| sessions/s | KV blocks | hit rate | TTFT | preemptions | utilisation |
|---|---|---|---|---|---|
| 2.0 | 4 000 | 0.486 | **168.9 s** | 174 634 | 1.000 |
| 2.0 | 40 000 | 0.801 | **0.020 s** | 0 | 0.471 |
| 3.0 | 40 000 | 0.801 | 0.023 s | 0 | 0.636 |

Same arrival rate, same engine, same cost model. Ten times the memory, and the
time to first token falls by a factor of **eight thousand** — and at half again
the load the big-pool system is still idle.

The engine did not run out of compute. It ran out of compute *because* it ran
out of memory.

## The loop

```
    memory is tight
          │
          ▼
 prefixes get evicted ──────┐
          │                 │
          ▼                 │
   turns miss, and a        │
   miss recomputes the      │
   whole context            │
          │                 │
          ▼                 │
  each turn costs the       │
  engine more              │
          │                 │
          ▼                 │
  turns stay resident       │
  longer, holding KV ───────┘
```

Nothing in this loop is a bug. Every step is the system behaving exactly as
designed. The loop has two stable states — one where prefixes survive and work
is cheap, and one where they do not and it is not — and the load at which it
falls out of the first is not where utilisation says it should be.

The 174 634 preemptions in the bottom row are the loop running: requests being
thrown out of the batch, their blocks freed, and their work recomputed when
they come back.

## This is not an artefact of the model

`examples/replay/vllm_replay.seq` is this same structure fitted to an A100 running
Qwen3-8B, replaying a measured 333-session trace. The measured replica
collapsed between a 3.0 s and a 2.5 s session spacing; the program predicted a
mean TTFT of 39.1 s against 34.6 s measured, and a full-hit rate of 0.192
against 0.216.

Better: the program predicted the *fix* before it was run. Pinning a waiting
request's prefix so that it cannot be evicted while it queues — one line, the
program without `admit via engine` — was predicted to take the 2.5 s replay
from 39.1 s to 0.888 s. Measured on the real A100 engine afterwards: 34.6 s →
0.878 s.

That pre-registered prediction is what a specification is *for*. You do not
run the experiment to find out what happens; you run it to find out whether
you were right.

## What to try

The interesting question is not where the cliff is but what moves it:

```bash
# more memory
seq-lang run docs/tutorial/programs/05-engine.seq --set Lambda=1.8 --set blocks=8000
# fewer concurrent requests, so each one holds memory for less time
seq-lang run docs/tutorial/programs/05-engine.seq --set Lambda=1.8 --set max_seqs=8
# shorter thinking time, so prefixes are reused before they are evicted
seq-lang run docs/tutorial/programs/05-engine.seq --set Lambda=1.8 --set Z=1
```

---

That is the language. Next: [a real system written in
it](../case-study-vllm.md), or [the reference](../reference/cheatsheet.md).
