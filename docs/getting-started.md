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
cargo install --git https://github.com/servingQ/serQ --tag v0.1.3 --locked --root ~/.local
```

As a dependency, pin a tag:

```toml
serq = { git = "https://github.com/servingQ/serQ", tag = "v0.1.3" }
```

## Run your first program

```bash
serq run examples/single-turn/mg1.sq --horizon 250000 --warmup 25000 --seed 1
```

```text
run: horizon 250000 end 250000 warmup 25000 seed 1 events 397862 arrivals 198931 ended 179177 turns 179176 mean live 3.899

observe    count    mean   95% CI    cv2      p99
--------  ------  ------  -------  -----  -------
response  179177  4.8967  ±0.2294  1.021  22.8477
wait      179177  3.8991  ±0.2263  1.543  21.6881
service   179177  0.9975  ±0.0051  1.002   4.5911

stage  number   util    done    thru    wait  service  iters
-----  ------  -----  ------  ------  ------  -------  -----
svc     3.899  0.794  179177  0.7963  3.8991   0.9975      0
```

That is an M/M/1 queue at 80 % utilisation. The closed form says the response
time is \(S/(1-\rho) = 5.0\) seconds and the mean number in system is
\(\rho/(1-\rho) = 4.0\); the run says `4.8967 ±0.2294` and `3.899`. The
confidence interval covers the theoretical mean.

## Reading a report

The report starts with run settings, followed by observations and resource statistics.

### The run line

```text
run: horizon 250000 end 250000 warmup 25000 seed 1 events 397862 arrivals 198931 ended 179177 ...
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

## Changing a program without editing it

A program constructs its deployment inside `fn main()`. It explicitly declares
external inputs through the standard `args` library:

```serq
use "std/args";

fn main() {
  let Lambda = args.number("Lambda", 0.3);
  // The deployment, workload, session and run settings go here.
}
```

Pass program inputs after `--`, and interpreter run settings before it.
Ordinary `let` constants cannot be changed from the command line:

```bash
serq run examples/multi-turn/vllm.sq --seed 2 --horizon 3000 -- --Lambda 0.3
```

`--json` prints the same report as JSON, and `--dump DIR` writes every
observation as `DIR/<name>.csv` with `time,session,turn,value` — one row per
sample, which is what you pair against a measured run.

## Checking without running

```bash
serq check examples/multi-turn/replica.sq
# OK: 3 pool(s), 2 stage(s), 20 attribute(s), 12 block(s)
```

`check` parses the program, resolves names and validates it without running
the simulation.

For JSON input and output, see the [CLI reference](reference/cli.md).
The [IR reference](ir.md) describes the compiled program format.

---

Next: [the tutorial](tutorial/index.md).
