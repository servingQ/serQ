# 6. The cliff

Sweep the arrival rate to find where latency rises sharply, then vary memory
to see how cache pressure affects that threshold.

## The sweep

```bash
for L in 1.5 1.7 1.8 1.9 2.0; do
  serq run docs/tutorial/programs/05-engine.sq --horizon 20000 --warmup 2000 --seed 1 --set Lambda=$L --json
done
```

| sessions/s | hit rate | TTFT (s) | preemptions | `stuck` | utilisation |
|---|---|---|---|---|---|
| 1.5 | 0.491 | 0.115 | 5,631 | 101 | 0.579 |
| 1.7 | 0.416 | 0.235 | 19,884 | 320 | 0.713 |
| 1.8 | 0.393 | 0.493 | 38,690 | 652 | 0.793 |
| 1.9 | 0.381 | 1.482 | 67,731 | 1,079 | 0.883 |
| 2.0 | 0.380 | 8.178 | 108,787 | 1,635 | 0.974 |

Between 1.9 and 2.0 sessions/s, a 5% load increase raises observed TTFT
from 1.48 s to 8.18 s. These seed-1 runs also report `stuck` sessions:
requests preempted again without progress. The latency samples therefore
do not describe all offered traffic. Inspect these counters before using
a mean for capacity planning, and repeat the experiment across seeds.

## Is it just saturation?

To separate compute saturation from memory pressure, run the same loads
with ten times the KV pool and the same cost model:

```bash
serq run docs/tutorial/programs/05-engine.sq --horizon 20000 --warmup 2000 --seed 1 --set Lambda=2.0 --set blocks=40000
serq run docs/tutorial/programs/05-engine.sq --horizon 20000 --warmup 2000 --seed 1 --set Lambda=3.0 --set blocks=40000
```

| sessions/s | KV blocks | hit rate | TTFT (s) | `stuck` | utilisation |
|---|---|---|---|---|---|
| 2.0 | 4,000 | 0.380 | 8.178 | 1,635 | 0.974 |
| 2.0 | 40,000 | 0.800 | 0.020 | 0 | 0.470 |
| 3.0 | 40,000 | 0.800 | 0.023 | 0 | 0.635 |

At 2.0 sessions/s, the larger pool reduces observed TTFT from 8.18 s to
0.020 s. Both larger-pool runs have no preemptions or `stuck` sessions.
Memory capacity changes the amount of recomputation, even with the same
arrival rate and engine cost model.

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

Eviction and recomputation can reinforce each other: longer service keeps
more KV resident, leaving less room for cached prefixes. The sweep shows a
sharp change in observed latency, but does not establish multiple stable states.

For comparison with a measured A100 deployment and a cache-policy
experiment, see the [vLLM use case](../use-cases/vllm.md).

## What to try

Vary one parameter at a time:

```bash
# more memory
serq run docs/tutorial/programs/05-engine.sq --horizon 20000 --warmup 2000 --seed 1 --set Lambda=1.8 --set blocks=8000
# fewer concurrent requests
serq run docs/tutorial/programs/05-engine.sq --horizon 20000 --warmup 2000 --seed 1 --set Lambda=1.8 --set max_seqs=8
# shorter thinking time
serq run docs/tutorial/programs/05-engine.sq --horizon 20000 --warmup 2000 --seed 1 --set Lambda=1.8 --set Z=1
```

---

Next: [a real system written in
it](../use-cases/vllm.md), or [the reference](../reference/cheatsheet.md).
