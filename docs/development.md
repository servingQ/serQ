# Working with a program

Check a program, inspect its deployment, then run experiments with explicit
parameters and repeatable workloads. For formal verification, see
[the Lean model](lean.md).

## In the simulator

### Write, check, look

```bash
serq check examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq  # validate without running
serq draw  examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --format svg --out my.svg
serq run   examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --horizon 2000 --warmup 200 --seed 1
```

`check` validates the program without simulating it. The deployment view
shows which stages each hold spans, helping you check resource lifetimes
before running an experiment ([visualization](visualization/index.md)).

The engine and the client are two files: the program is the deployment,
its pools, stages and `server` block, and the workload file, given with
`--workload`, is the traffic ([paired files](language.md#paired-files)).
A question about traffic is another workload file run on the same
deployment, and a test holds the engine fixed
([different workloads](use-cases/workloads.md)); `examples/pairs.json`
names the workload each example runs.

### Read the report

The report lists observations and resource statistics. This excerpt shows
the run settings, observations and pools:

```
$ serq run examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --horizon 300 --warmup 30
run: horizon 300 end 300 warmup 30 seed 1 events 150579 arrivals 95 ended 86 turns 779 mean live 7.387

observe       count       mean     95% CI    cv2         p99
------------  -----  ---------  ---------  -----  ----------
vllm.reused     779  6525.5558  ±702.3442  0.642  23488.0000
vllm.prefill    779   685.2439   ±56.1267  1.068   2883.0000

client    count       mean      95% CI    cv2        p99
--------  -----  ---------  ----------  -----  ---------
ttft        779    0.01428    ±0.00122  1.070    0.05924
tpot        776  0.0002530  ±0.0000354  3.742  0.0007055
response    779    0.05994    ±0.00397  0.567    0.19769

…

pool    used    cached  queue  holders    wait  admits  evict(n)  evict(u)  preempt  spill  rej  stuck
----  ------  --------  -----  -------  ------  ------  --------  --------  -------  -----  ---  -----
kv    1251.5  144310.0  0.000    0.171     NaN     798        76    545120        0      0    0      0
reqs     0.2       0.0  0.001    0.171  0.0005     798         0         0        0      0    0      0
```

`observe` rows are the program's own, and its engines' records
(`vllm.reused`, the tokens an admission read from the cache;
`vllm.prefill`, the tokens it computed): each of the program's is whatever
its `observe` line says, so read that line before comparing the number with
a measurement.
`client` rows are what each turn's client saw, the same in every program:
`ttft` from sending the turn to its first output token, `tpot` the time per
output token after the first, `response` from sending to the answer
([what the client sees](language.md#what-the-client-sees)). Before reading the means, check the pool columns: `rej`
(requests that can never fit), `stuck` (sessions preempted again without
progress, a livelock) and `preempt`. A run with non-zero `stuck` has a
mean over the sessions that got through, not over the traffic.

`--json` prints the same report as JSON, and `--dump DIR` writes one CSV per
`observe` with columns `time,session,turn,value`, for a distribution, a
per-turn breakdown, or a statistic the report does not compute
([CLI](reference/cli.md)).

### Sweep a parameter

An input declared with `args.number` is set from the command line, so a
sweep is a loop over `--name value`, with a few seeds per point:

```bash
for lam in 0.3 0.6 0.9; do
  for seed in 1 2 3; do
    serq run examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --horizon 2000 --warmup 200 --Lambda $lam --seed $seed --json \
      | jq -r --arg l $lam --arg s $seed \
          '[$l, $s, .client.ttft.mean, (.pools[] | select(.name=="kv") | .preemptions)] | @tsv'
  done
done
```

Compare both TTFT and preemptions across seeds. Near saturation, different
seeds can produce different congestion histories; a confidence interval
within one run does not describe that spread. Check rejections and `stuck`
as well before interpreting a lower latency as an improvement.

### Compare two designs

Run both designs with the same seeds and arrival settings. Workload draws
use streams keyed by session and turn, so changing event order does not
reassign one session's draws to another. Draws in session statements follow
each session's execution order; changing that order or an expression's
inputs can still change the workload experienced by the designs.

For an explicit common workload, use `trace "file.csv" ordered` and compare
per-session observations. See the [vLLM use case](use-cases/vllm.md) for a
comparison of admission policies on the same trace.

### Calibrate

The parameters of a program are measurements, and the program says where each
one comes from:

- **An engine cost model.** Fit `cost` to measured iterations, as
  `examples/replay/vllm_replay.sq` does for the A100: an expression in `tokens`,
  `decoders`, `prefilled`, `kv_decode`, `kv_prefill`, `attention`.
- **Traffic from a trace.** `trace "file.csv"` in the workload file's
  `workload` replays sessions turn by turn, the path read beside that file
  (`examples/workloads/vllm_replay.sq`). The columns are
  `session,turn,new,out,think,forced` (`examples/replay/data/`), and
  `--trace F` swaps the file without editing the workload.
- **A quantity the program cannot compute itself.** For example, the subagent
  wait `W` of `examples/workloads/subagents.sq` (an input of the workload
  `examples/multi-turn/vllm.sq` runs under) is taken from the program's own
  output. Run the program, compute the statistic from `--dump`, set it with
  `--W …` (the example exposes `W` through `args.number`), and repeat
  until it stops moving. Write the fixed point into that input's default
  with a comment that says how it was obtained.

## In Lean

Use [claims](lean.md#claims) to state and prove properties of supported
programs. The [Lean guide](lean.md) explains the build, the accepted fragment
and the distinction between a tested scenario and a result over all paths.
