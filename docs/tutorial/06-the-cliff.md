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
for H in 20000 40000 80000 160000; do
  for L in 1.5 1.7 1.8 1.9 2.0; do
    serq run docs/tutorial/programs/05-engine.sq --horizon $H --warmup 2000 --seed 1 --set Lambda=$L --json
  done
done
```

| sessions/s | TTFT (s) at 20 000 | 40 000 | 80 000 | 160 000 | hit rate at 160 000 |
|---|---|---|---|---|---|
| 1.5 | 0.145 | 0.130 | 0.128 | 0.132 | 0.459 |
| 1.7 | 0.404 | 0.327 | 0.295 | 0.295 | 0.335 |
| 1.8 | 1.276 | 0.815 | 0.670 | 0.615 | 0.252 |
| 1.9 | 4.457 | 2.682 | 2.175 | 1.815 | 0.145 |
| 2.0 | 17.921 | 26.486 | 33.251 | 70.388 | 0.005 |

Up to 1.7 the TTFT is a level: from 80 000 s on, a run twice as long gives
the same. At 1.8 and 1.9 it falls as the run grows and has not settled by
160 000 s (0.615 s and 1.815 s there): the 20 000 s runs caught an early
excursion that a longer run dilutes, and at 80 000 s seeds 1–4 give
0.60–0.78 s at 1.8 and 1.6–3.4 s at 1.9. At 2.0 it keeps growing with the
run length, so 17.92 s is one moment of a queue without bound, not a level.

The cliff is between 1.9 and 2.0 sessions/s, and latency rises steeply
before it: at 160 000 s the TTFT doubles from 1.7 to 1.8 and triples from
1.8 to 1.9. The hit rate at 160 000 s falls from 0.46 at 1.5 to 0.15 at
1.9, and to 0.005 at 2.0.

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
