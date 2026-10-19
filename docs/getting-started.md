# Getting started

## Install

serQ is one Rust crate, `serq`: a library (`serq`) and a CLI (`serq`).
From Python, `pip install pyserq` (a binding of the crate) compiles, runs
and draws a program in process ([pyserq](python.md)).

```bash
git clone https://github.com/servingQ/serQ && cd serQ
cargo install --path . --locked
serq --help
```

Or install the CLI straight from a release tag:

```bash
cargo install --git https://github.com/servingQ/serQ --tag v0.1.6 --locked --root ~/.local
```

As a dependency, pin a tag:

```toml
serq = { git = "https://github.com/servingQ/serQ", tag = "v0.1.6" }
```

## Run your first program

A program is the deployment; the workload, how sessions arrive and what
each turn sends, is a file of its own, given when the program runs. The
examples' workloads are in `examples/workloads/`, and `examples/pairs.json`
names the one each program runs.

```bash
serq run examples/single-turn/mg1.sq --workload examples/workloads/mg1.sq --horizon 250000 --warmup 25000 --seed 1
```

```text
run: horizon 250000 end 250000 warmup 25000 seed 1 events 397859 arrivals 198931 ended 179174 turns 179176 mean live 3.862

observe        count    mean   95% CI    cv2      p99
------------  ------  ------  -------  -----  -------
wait          179174  3.8499  ±0.1644  1.497  20.9580
service_time  179174  0.9993  ±0.0051  0.995   4.6129

client     count   mean  95% CI    cv2     p99
--------  ------  -----  ------  -----  ------
response  179174  4.849  ±0.168  0.985  22.108

state     stays    mean     p50      p99  number
-------  ------  ------  ------  -------  ------
waiting  142452  4.8424  3.3835  21.9678   3.066
running  179174  0.9993  0.6955   4.6061   0.796

stage  number   util    done    thru    wait  service  iters
-----  ------  -----  ------  ------  ------  -------  -----
svc     3.862  0.796  179174  0.7963  3.8499   0.9993      0
```

That is an M/M/1 queue at 80 % utilisation. The closed form says the response
time is \(S/(1-\rho) = 5.0\) seconds and the mean number in system is
\(\rho/(1-\rho) = 4.0\); the run says `4.849 ±0.168` and `3.862`. The
confidence interval covers the theoretical mean.

## Reading a report

The report starts with run settings, followed by observations and resource statistics.

### The run line

```text
run: horizon 250000 end 250000 warmup 25000 seed 1 events 397859 arrivals 198931 ended 179174 ...
```

`horizon` is simulated time, `warmup` excludes the initial period from
measured statistics, and `seed` initializes the random streams. `events`
and `arrivals` count the whole run; `ended` counts sessions that finish
after warm-up, and `turns` counts turns started after warm-up. Each arrival
in this single-turn workload starts one turn automatically. The counts can
differ because starts and completions cross the measurement boundaries.

### `observe`

One row per `observe` name in the program. `mean` with a batch-means 95 %
confidence interval, the squared coefficient of variation `cv2`, and the 99th
percentile.

The interval describes uncertainty within this run. Compare multiple seeds
and longer horizons before drawing conclusions, especially near saturation.
Below 40 samples, the printed CI is `±inf`.

### Stages and pools

Per stage: the time-average number present, utilisation, completions,
throughput, mean wait and mean service. `iters` is non-zero only for `step`
stages.

Per pool: the time-average units used and cached, queue length, holders, mean
queue wait, and counters for admissions, evictions, preemptions, spills,
rejections and `stuck` (sessions preempted again without progress since
their previous preemption, which a run would otherwise hide).

`--json` prints the same report as JSON, and `--dump DIR` writes every
observation as `DIR/<name>.csv` with `time,session,turn,value` — one row per
sample, which is what you pair against a measured run. `--log FILE` writes
what each session did, when ([the run log](reference/log.md)): the run as a
replay rather than a summary.

## Checking without running

```bash
serq check examples/multi-turn/replica.sq --workload examples/workloads/replica.sq
# OK: 3 pool(s), 2 stage(s), 24 attribute(s), 14 block(s)
```

`check` parses the program and its workload, resolves names and validates
them without running the simulation. Without `--workload` it stops at the
server: "the server needs a workload to run".

For JSON input and output, see the [CLI reference](reference/cli.md).
The [IR reference](ir.md) describes the compiled program format.

---

Next: [the tutorial](tutorial/index.md).
