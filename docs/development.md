# Working with a program

Check a program, inspect its deployment, then run experiments with explicit
parameters and repeatable workloads. For formal verification, see
[the Lean model](lean.md).

## In the simulator

### Write, check, look

```bash
serq check examples/multi-turn/vllm.sq  # validate without running
serq draw  examples/multi-turn/vllm.sq --format svg --out my.svg
serq run   examples/multi-turn/vllm.sq --horizon 2000 --warmup 200 --seed 1
```

`check` validates the program without simulating it. The deployment view
shows which stages each hold spans, helping you check resource lifetimes
before running an experiment ([visualization](visualization/index.md)).

Keep the engine and the client apart, as the programs in `examples/` do: the
pools, the stages and a `server` block for the deployment, and `workload` for
the traffic ([the two sides](language.md#the-two-sides)). Then a question
about traffic is an edit of `workload` alone, and a test can hold the engine
fixed ([different workloads](use-cases/workloads.md)).

### Read the report

The report lists observations and resource statistics. This excerpt shows
the run settings, observations and pools:

```
$ serq run examples/multi-turn/vllm.sq --horizon 300 --warmup 30
run: horizon 300 end 300 warmup 30 seed 1 events 133894 arrivals 95 ended 87 turns 697 mean live 6.617

observe         count      mean    95% CI    cv2        p99
--------------  -----  --------  --------  -----  ---------
hit               697    0.8723   ±0.0223  0.147     1.0000
prefill_tokens    697  723.1133  ±53.2095  0.985  2890.0000
ttft              697    0.0151   ±0.0010  0.952     0.0578
response          697    0.0598   ±0.0032  0.602     0.2257

…

pool    used    cached  queue  holders    wait  admits  evict(n)  evict(u)  preempt  spill  rej  stuck
----  ------  --------  -----  -------  ------  ------  --------  --------  -------  -----  ---  -----
kv    1032.6  144255.8  0.000    0.153     NaN     719        70    500048        0      0    0      0
reqs     0.2       0.0  0.002    0.153  0.0006     719         0         0        0      0    0      0
```

The metrics are the program's own: `ttft` is whatever the `observe ttft = …`
line says it is, so read that line before comparing the number with a
measurement. Before reading the means, check the pool columns: `rej`
(requests that can never fit), `stuck` (sessions preempted again without
progress, a livelock) and `preempt`. A run with non-zero `stuck` has a
mean over the sessions that got through, not over the traffic.

`--json` prints the same report as JSON, and `--dump DIR` writes one CSV per
`observe` with columns `time,session,turn,value`, for a distribution, a
per-turn breakdown, or a statistic the report does not compute
([CLI](reference/cli.md)).

### Sweep a parameter

Every `let` can be overridden from the command line, so a sweep is a loop
over `--set`, with a few seeds per point:

```bash
for lam in 0.3 0.6 0.9; do
  for seed in 1 2 3; do
    serq run examples/multi-turn/vllm.sq --horizon 2000 --warmup 200 --set Lambda=$lam --seed $seed --json \
      | jq -r --arg l $lam --arg s $seed \
          '[$l, $s, .observes.ttft.mean, (.pools[] | select(.name=="kv") | .preemptions)] | @tsv'
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
- **Traffic from a trace.** `trace "file.csv"` in the `workload` replays
  sessions turn by turn. The columns are `session,turn,new,out,think,forced`
  (`examples/replay/data/`), and `--trace F` swaps the file without editing the
  program.
- **A quantity the program cannot compute itself.** For example, the subagent
  wait `W` of `examples/subagent/vllm_subagents.sq` is taken from the program's own
  output. Run the program, compute the statistic from `--dump`, set it with
  `--set W=…` (the example exposes `W` through `args.number`), and repeat
  until it stops moving. Write the fixed point into that input's default
  with a comment that says how it was obtained.

## In Lean

Use [claims](lean.md#claims) to state and prove properties of supported
programs. The [Lean guide](lean.md) explains the build, the accepted fragment
and the distinction between a tested scenario and a result over all paths.
