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
| 1.5 | 0.450 | 0.145 | 906 | 154 | 0.597 |
| 1.7 | 0.327 | 0.404 | 2,439 | 406 | 0.725 |
| 1.8 | 0.230 | 1.276 | 4,506 | 754 | 0.813 |
| 1.9 | 0.125 | 4.457 | 7,274 | 1,198 | 0.902 |
| 2.0 | 0.041 | 17.921 | 9,780 | 1,582 | 0.971 |

A mean from one run is a level only if a longer run gives about the same.
The same seed at longer horizons:

```bash
for H in 20000 40000 80000 160000 320000; do
  for L in 1.5 1.7 1.8 1.9 2.0; do
    serq run docs/tutorial/programs/05-engine.sq --horizon $H --warmup 2000 --seed 1 --set Lambda=$L --json
  done
done
```

| sessions/s | TTFT (s) at 20 000 | 40 000 | 80 000 | 160 000 | 320 000 | hit rate at 320 000 |
|---|---|---|---|---|---|---|
| 1.5 | 0.145 | 0.130 | 0.128 | 0.132 | 0.133 | 0.455 |
| 1.7 | 0.404 | 0.327 | 0.295 | 0.295 | 0.313 | 0.326 |
| 1.8 | 1.276 | 0.815 | 0.670 | 0.615 | 0.647 | 0.240 |
| 1.9 | 4.457 | 2.682 | 2.175 | 1.815 | 2.288 | 0.125 |
| 2.0 | 17.921 | 26.486 | 33.251 | 70.388 | 195.907 | 0.002 |

Up to 1.9 the TTFT has a level: about 0.13 s at 1.5, 0.30 s at 1.7,
0.63 s at 1.8 and 2.2 s at 1.9. The 20 000 s runs at 1.8 and 1.9 caught an
early excursion that a longer run dilutes. The level at 1.9 is reached
slowly and is noisy: at 320 000 s, seeds 1–3 give 2.29, 2.22 and 2.08 s,
each with a 95 % interval of ±0.5–0.6 s within its run (at 1.8 they give
0.65, 0.62 and 0.61 s). Only 2.0 has no level: its TTFT keeps growing with
the run length, so 17.92 s is one moment of a queue without bound.

The cliff is between 1.9 and 2.0 sessions/s, and latency rises steeply
before it: the level doubles from 1.7 to 1.8 and grows about 3.5 times
from 1.8 to 1.9. The hit rate at 320 000 s falls from 0.46 at 1.5 to 0.13
at 1.9, and to 0.002 at 2.0.

These runs also report `stuck` sessions: requests preempted again without
progress. The latency samples therefore do not describe all offered
traffic. Inspect these counters, and the run length, before using a mean
for capacity planning, and repeat the experiment across seeds.

## Is it just saturation?

To separate compute saturation from memory pressure, run the same loads
with ten times the KV pool and the same cost model:

```bash
serq run docs/tutorial/programs/05-engine.sq --horizon 20000 --warmup 2000 --seed 1 --set Lambda=2.0 --set blocks=40000
serq run docs/tutorial/programs/05-engine.sq --horizon 20000 --warmup 2000 --seed 1 --set Lambda=3.0 --set blocks=40000
```

| sessions/s | KV blocks | hit rate | TTFT (s) | `stuck` | utilisation |
|---|---|---|---|---|---|
| 2.0 | 4,000 | 0.041 | 17.921 | 1,582 | 0.971 |
| 2.0 | 40,000 | 0.800 | 0.020 | 0 | 0.470 |
| 3.0 | 40,000 | 0.800 | 0.023 | 0 | 0.633 |

At 2.0 sessions/s, the larger pool turns a queue that grows without bound
(17.92 s at this horizon) into a TTFT of 0.020 s. Both larger-pool runs have no preemptions or `stuck` sessions.
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
