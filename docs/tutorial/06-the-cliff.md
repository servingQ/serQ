# 6. The cliff

Sweep the arrival rate to find where latency rises sharply, then vary memory
to see how cache pressure affects that threshold.

## The sweep

```bash
for L in 1.5 1.7 1.8 1.9 2.0; do
  serq run docs/tutorial/programs/05-engine.sq --workload docs/tutorial/programs/workloads/05-engine.sq --horizon 20000 --warmup 2000 --seed 1 --Lambda $L --json
done
```

| sessions/s | from cache | TTFT (s) | preemptions | `stuck` | utilisation |
|---|---|---|---|---|---|
| 1.5 | 0.443 | 0.131 | 320 | 0 | 0.597 |
| 1.7 | 0.324 | 0.264 | 1,035 | 0 | 0.727 |
| 1.8 | 0.251 | 0.504 | 2,012 | 0 | 0.803 |
| 1.9 | 0.151 | 1.505 | 3,635 | 0 | 0.894 |
| 2.0 | 0.031 | 25.173 | 6,257 | 0 | 1.000 |

*From cache* is the share of the prompt tokens the cache supplied,
`llm.reused / (llm.reused + llm.prefill)` (the means of the report's two
`observe` rows); TTFT is the `client` section's mean, preemptions and
`stuck` the `kv` pool's, utilisation the `llm` stage's.

A mean from one run is a level only if a longer run gives about the same.
The same seed at longer horizons:

```bash
for H in 20000 40000 80000 160000 320000; do
  for L in 1.5 1.7 1.8 1.9 2.0; do
    serq run docs/tutorial/programs/05-engine.sq --workload docs/tutorial/programs/workloads/05-engine.sq --horizon $H --warmup 2000 --seed 1 --Lambda $L --json
  done
done
```

| sessions/s | TTFT (s) at 20 000 | 40 000 | 80 000 | 160 000 | 320 000 | from cache at 320 000 |
|---|---|---|---|---|---|---|
| 1.5 | 0.131 | 0.132 | 0.132 | 0.136 | 0.137 | 0.437 |
| 1.7 | 0.264 | 0.300 | 0.287 | 0.314 | 0.315 | 0.312 |
| 1.8 | 0.504 | 0.627 | 0.599 | 0.665 | 0.664 | 0.233 |
| 1.9 | 1.505 | 2.319 | 2.355 | 2.311 | 2.295 | 0.130 |
| 2.0 | 25.173 | 33.170 | 63.421 | 126.720 | 288.270 | 0.032 |

Up to 1.9 the TTFT has a level: about 0.14 s at 1.5, 0.31 s at 1.7,
0.6 s at 1.8 and 2.0 s at 1.9 (the last two the mean of seeds 1–3 at
320 000 s). The level at 1.9 is noisy: at 320 000 s, seeds 1–3 give 2.30,
1.70 and 1.94 s, each with a 95 % interval of ±0.3–0.5 s within its run
(at 1.8 they give 0.66, 0.56 and 0.57 s). With seed 1 the 20 000 s runs
at 1.8 and 1.9 read below the level: the queue's long excursions are
rare, and a short run may not sample one; another seed may read above
it. Only 2.0 has no level: its TTFT
keeps growing with the run length, so 25.17 s is one moment of a queue
without bound.

The cliff is between 1.9 and 2.0 sessions/s, and latency rises steeply
before it: the level doubles from 1.7 to 1.8 and grows about 3.3 times
from 1.8 to 1.9. The share of prompt tokens from cache at 320 000 s falls
from 0.44 at 1.5 to 0.13 at 1.9, and to 0.03 at 2.0.

No run here reports a `stuck` session (a request preempted again without
progress); when one does, its latency samples do not describe all
offered traffic. Inspect these counters, and the run length, before using
a mean for capacity planning, and repeat the experiment across seeds.

Measured 2026-10-14 on serQ 1b0a47b with the paired-workload change (the
workload a file of its own), by the commands on this page (seeds 2 and 3:
`--seed 2`, `--seed 3` at `--horizon 320000`).

## Is it just saturation?

To separate compute saturation from memory pressure, run the same loads
with ten times the KV pool and the same cost model:

```bash
serq run docs/tutorial/programs/05-engine.sq --workload docs/tutorial/programs/workloads/05-engine.sq --horizon 20000 --warmup 2000 --seed 1 --Lambda 2.0 --blocks 40000
serq run docs/tutorial/programs/05-engine.sq --workload docs/tutorial/programs/workloads/05-engine.sq --horizon 20000 --warmup 2000 --seed 1 --Lambda 3.0 --blocks 40000
```

| sessions/s | KV blocks | from cache | TTFT (s) | preemptions | utilisation |
|---|---|---|---|---|---|
| 2.0 | 4,000 | 0.031 | 25.173 | 6,257 | 1.000 |
| 2.0 | 40,000 | 0.832 | 0.020 | 0 | 0.471 |
| 3.0 | 40,000 | 0.832 | 0.023 | 0 | 0.635 |

At 2.0 sessions/s, the larger pool turns a queue that grows without bound
(25.17 s at this horizon) into a TTFT of 0.020 s. Both larger-pool runs have no preemptions.
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
serq run docs/tutorial/programs/05-engine.sq --workload docs/tutorial/programs/workloads/05-engine.sq --horizon 20000 --warmup 2000 --seed 1 --Lambda 1.8 --blocks 8000
# fewer concurrent requests
serq run docs/tutorial/programs/05-engine.sq --workload docs/tutorial/programs/workloads/05-engine.sq --horizon 20000 --warmup 2000 --seed 1 --Lambda 1.8 --max_seqs 8
# shorter thinking time
serq run docs/tutorial/programs/05-engine.sq --workload docs/tutorial/programs/workloads/05-engine.sq --horizon 20000 --warmup 2000 --seed 1 --Lambda 1.8 --Z 1
```

---

Next: [a real system written in
it](../use-cases/vllm.md), or [the reference](../reference/cheatsheet.md).
