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

Between 1.9 and 2.0 sessions/s, a 5% load increase raises observed TTFT
from 4.46 s to 17.92 s, and over the sweep the hit rate falls from 0.45
to 0.04. At 1.9 the TTFT is a level: a run twice as long gives about the same.
At 2.0 it keeps growing with the run length, so 17.92 s is one moment of a
queue without bound, not a level. These seed-1 runs also report `stuck` sessions:
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
