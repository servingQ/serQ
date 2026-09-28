# Getting started

## Build

seQ is one Rust crate, `seq-lang`: a library (`seq`) and a CLI (`seq-lang`).

```bash
git clone https://github.com/vrvrv/seQ && cd seQ
cargo build --release
./target/release/seq-lang --help
```

Or install the CLI straight from a release tag:

```bash
cargo install --git https://github.com/vrvrv/seQ --tag v0.1.0-rc0 --locked --root ~/.local
```

As a dependency, pin a tag:

```toml
seq = { package = "seq-lang", git = "https://github.com/vrvrv/seQ", tag = "v0.1.0-rc0" }
```

## Run your first program

```bash
seq-lang run programs/mg1.seq
```

```text
run: horizon 250000 warmup 25000 seed 1 events 397859 arrivals 198931 ended 179184 turns 0 mean live 3.877
observe        count        mean      95% CI      cv2       p99
  sojourn      179184      4.8684 ±0.2109      0.955   21.4103
  wait         179184      3.8689 ±0.2074      1.446   20.3125
  service      179184      0.9995 ±0.0048      1.005    4.5948
stage          number   util    done   thru      wait   service  iters
  svc            3.877  0.796  179184  0.7964    3.8689    0.9995      0
```

That is an M/M/1 queue at 80 % utilisation. The closed form says the sojourn
time is \(S/(1-\rho) = 5.0\) seconds and the mean number in system is
\(\rho/(1-\rho) = 4.0\); the run says `4.8684 ±0.2109` and `3.877`. The
confidence interval covers the answer, which is the point of reporting one.

## Reading a report

Three blocks, always in this order.

### The run line

```text
run: horizon 250000 warmup 25000 seed 1 events 397859 arrivals 198931 ended 179184 ...
```

`horizon` is simulated seconds, `warmup` the seconds discarded before anything
is recorded, `seed` the RNG seed. `events` is how much work the interpreter
did; `arrivals`, `ended` and `turns` count sessions and turns after warm-up
(`turns 0` above because `mg1.seq` has no `turn` statement — one request per
session).

### `observe`

One line per `observe` name in the program. `mean` with a batch-means 95 %
confidence interval, the squared coefficient of variation `cv2`, and the 99th
percentile.

!!! tip "The CI is the first thing to look at"
    `±0.2109` on a mean of `4.8684` means the run is long enough to say
    something. A CI as wide as the mean means it is not — raise `--horizon`.
    Below 40 samples the CI is reported as `NaN`.

### Stages and pools

Per stage: the time-average number present, utilisation, completions,
throughput, mean wait and mean service. `iters` is non-zero only for `step`
stages.

Per pool: the time-average units used and cached, queue length, holders, mean
queue wait, and counters for admissions, evictions, preemptions, spills and
rejections.

## Changing a program without editing it

Every `let` constant is an override:

```bash
seq-lang run programs/agentic.seq --set N=32 --set C=3e5 --set maxctx=1.5e5
seq-lang run programs/vllm.seq --seed 2 --horizon 3000
```

`--json` prints the same report as JSON, and `--dump DIR` writes every
observation as `DIR/<name>.csv` with `time,session,turn,value` — one row per
sample, which is what you pair against a measured run.

## Checking without running

```bash
seq-lang check programs/replica.seq
# OK: 3 pool(s), 2 stage(s), 19 attribute(s), 12 block(s)
```

`check` parses, resolves every name and folds the constants. It is what
`make check` runs over every program in `programs/`.

## The IR

A seQ program's definition is not its text — it is the **IR**, a closed
versioned data structure ([reference](ir.md)). The text syntax is one frontend.

```bash
seq-lang ir programs/vllm.seq > vllm.json   # compile text to IR
seq-lang run vllm.json --seed 3             # run the IR directly
```

This matters more than it looks: the Lean model is generated from the IR, the
vLLM oracle tests read IR files, and tools that know what they want to run
build the IR as data instead of generating text.

---

Next: [the tutorial](tutorial/index.md).
