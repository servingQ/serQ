# Working with a program

A serQ program has two consumers besides its reader. The **simulator**
(`serq run`) executes it as a discrete-event system and reports what a
deployment under that traffic does. **Lean** (the `lean/` package,
[The Lean model](lean.md)) executes the same program's [IR](ir.md) in an executable
semantics and states, as theorems checked by the kernel, what it does. This
page covers how to use each one and what each one needs from a program.

```
program.sq ──serq ir──▶ IR (JSON) ──serq run──▶ report, samples     (simulator)
                              │
                              └──gen_lean_oracle.py──▶ lean/Serq/Oracle.lean ──lake build──▶ theorems  (Lean)
```

The IR sits in the middle, not the text. The simulator reads it too
(`serq run` accepts a `.json` as well as a `.sq`), so the two consumers
never interpret different programs.

## In the simulator

### Write, check, look

```bash
serq check examples/my.sq                       # parse, resolve names, lint
serq draw  examples/my.sq --format svg --out my.svg
serq run   examples/my.sq
```

`check` is what `make check` runs over every `examples/*/*.sq`, and it also
catches the two lints (a stale header read, a draw written as a test,
[language](language.md) §3). Draw the program before trusting a run. A hold
is drawn around the stations it spans - its pool's enclosure, or the frame of
the one station it reaches - so a hold that closes one statement too early
leaves a station outside the box
([visualization](visualization/index.md)).

Keep the engine and the client apart, as the programs in `examples/` do: the
pools, the stages and a `server` block for the deployment, and `workload` for
the traffic ([the two sides](language.md#the-two-sides)). Then a question
about traffic is an edit of `workload` alone, and a test can hold the engine
fixed ([different workloads](use-cases/workloads.md)).

### Read the report

A run prints its run line and then its tables: the `observe`s, the stages,
the step stages' iterations and the pools, each column as wide as its widest
entry:

```
$ serq run examples/multi-turn/vllm.sq --horizon 300 --warmup 30
run: horizon 300 end 300 warmup 30 seed 1 events 133894 arrivals 95 ended 87 turns 697 mean live 6.617

observe         count      mean    95% CI    cv2        p99
--------------  -----  --------  --------  -----  ---------
hit               697    0.8723   ±0.0223  0.147     1.0000
prefill_tokens    697  723.1133  ±53.2095  0.985  2890.0000
ttft              697    0.0151   ±0.0010  0.952     0.0578
response          697    0.0598   ±0.0032  0.602     0.2257

stage   number   util  done    thru    wait  service   iters
------  ------  -----  ----  ------  ------  -------  ------
engine   0.153  0.139  1392  5.1556  0.0000   0.0296  133174
tool     6.463  1.000   608  2.2519  0.0000   2.8874       0

step    prefill only  decode only  mixed   idle  decodes  decode batch  decode step   itl p50   itl p99
------  ------------  -----------  -----  -----  -------  ------------  -----------  --------  --------
engine         0.033        0.102  0.005  0.861    0.115         1.076     0.000225  0.000211  0.000253

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
    serq run examples/multi-turn/vllm.sq --set Lambda=$lam --seed $seed --json \
      | jq -r --arg l $lam --arg s $seed \
          '[$l, $s, .observes.ttft.mean, (.pools[] | select(.name=="kv") | .preemptions)] | @tsv'
  done
done
```

| `Lambda` | seed 1 TTFT (s) / preemptions | seed 2 | seed 3 |
|---|---|---|---|
| 0.3 | 0.018 / 0 | 0.016 / 0 | 0.015 / 0 |
| 0.6 | 0.109 / 31 | 0.600 / 227 | 0.085 / 40 |
| 0.9 | 23.9 / 124 | 24.6 / 127 | 18.9 / 153 |

At 0.6 the three seeds disagree by a factor of 7. The deployment is at its
cliff, and whether one run falls off depends on the draws. The report's 95 %
CI is within one run and does not show this. Near a load where preemptions
start, report the spread across seeds, not one seed's interval.

### Compare two designs

Write the two designs as one program with a `let` that selects between them,
or as two programs that differ in one line, and run both on the same seeds.
Arrivals, the workload, the sessions and eviction draw from separate random
streams ([language](language.md) §3), but the workload and session streams
are each shared by every session and consumed in event order
(`src/engine/interp.rs`, `rng_wl`, `rng_session`). So with the same seed the two
designs get the same arrival times. Once one design changes when things
happen, the turn draws (`n`, `o`, `more`) and the tool times go to different
sessions, and the traffic is no longer paired. To give both designs exactly
the same sessions, replay a trace with `trace "file.csv" ordered`. The
pre-registered prediction in the [vLLM use case](use-cases/vllm.md) is this
kind of comparison: the same program with and without `admit via engine`.

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
  `--set`, and repeat until it stops moving. Write the fixed point into the
  `let` with a comment that says how it was obtained.

### Add a program to the repository

Put it in the `examples/` directory of its workload (`single-turn/`, `multi-turn/`, `subagent/`, `pd-disaggregation/`, `replay/`); names are unique across them. `make check` then links it and draws it
in both formats. Add a row to [language](language.md) §5 saying what it
models and what it is checked against, and a test that checks that claim.
A program that cites vLLM cites it as `file.py:lines`, and
`scripts/check_citations.py` holds the citation to the pinned source:
`make check` resolves every cited range in the pinned `ref/vllm` and hashes
its text (`tools/citations.json`), so a range that moved, a file that is
gone or a citation nobody recorded fails the build. That check cannot see
upstream move; `.github/workflows/citation-drift.yml` asks that daily, by
looking for each range's pinned text in the same files at
`vllm-project/vllm@main` (`scripts/check_citations.py --tip`). It gates no
pull request: it says the cited code is gone, not that the scheduler now
computes something else.

Some link errors come from bugs this repository shipped: a hold header that
reads a `set` bound before the session queued, a `branch` whose constant
guard is a probability, a context variable read outside the moment that
supplies it. `tests/lints.rs` holds them, `no_false_positives_on_the_corpus`
holds the reference programs to linking, and `make check` links every
program in `examples/`, so a new program that trips one is either the bug
it describes or a false positive to fix in the lint.

## In Lean

### What Lean gives a program

There are three kinds of result, and a program meets them differently:

| | holds for | where |
|---|---|---|
| properties of the semantics | the model, not one program | `Serq/Core.lean`: `SerqLang.Step.invariant` (every command of the pool model keeps `allocated + cached ≤ cap`); `Serq/Serve.lean`: `SerqLang.Serve.serve_eq_decode_first` (without a per-request chunk cap, serving in admission order is serving decode-first) |
| a program's outcome on a scenario | one IR file and one workload | `Serq/Oracle.lean`, generated: one theorem per scenario, proved by `decide +kernel` |
| a real-valued model of a deployment | a hand-written `Route` | `serving-queue-theory`'s `Deployments.lean`, which requires this package: `colocatedReplica` (`examples/multi-turn/replica.sq`), `disaggregatedReplica` (the lecture notes' store-and-forward replica; no serQ program), with their well-formedness |

The first kind needs nothing from a program. It is about the pool model and
the serving order, and it is not yet connected to the executable semantics
that runs programs ([The Lean model](lean.md#what-is-not-covered-yet)). The
second is how a specific program is checked. The third is written by hand in
serving-queue-theory, so a change to the program it describes has to be
repeated there when that repository moves its pin of this package.

### From a program to a theorem

A generated theorem says: the executable semantics (`Exec.runW`, in
`lean/Serq/Exec.lean`) runs this program on this deployment and these sessions, and
gives these observations. For example, for `tools/oracle/hol.ir.json`:

```lean
theorem vllm_hol :
    outcome ⟨[⟨16, 1, true⟩, ⟨160, 16, false⟩], 1024, 0⟩ 25
      ⟨[[(8, 96), (9, 10), (10, 0)], [(8, 96), (9, 10), (10, 0)], [(8, 16), (9, 3), (10, 0)]], [], none, 0⟩ =
    ([(0, 1), (1, 11), (2, 11)], [(0, 10), (1, 20), (2, 13)], 0) := by
  decide +kernel
```

The right-hand side is not what serQ computed but what the real vLLM scheduler
did on the same scenario (`tools/oracle/hol.out.json`). So the theorem ties
three things together: the program, the Lean semantics, and vLLM. The Rust
interpreter is held to the same answers by `tests/vllm_oracle.rs`, from the
same IR file.

The steps:

1. **serQ:** compile the program to IR once per scenario. For the request
   scenarios, `tests/vllm_oracle.rs` does this: it compiles
   `examples/oracle/vllm_request.sq` with each scenario's engine as `let`
   overrides and its requests as explicit sessions. `make oracle-ir` writes
   `tools/oracle/<name>.ir.json`, and `make check` fails if a committed file
   is stale.
2. **Lean:** `make oracle-ir` also runs `scripts/gen_lean_oracle.py`, which
   reads `tools/oracle/*.ir.json` with the matching `*.json` and
   `*.out.json` and writes `lean/Serq/Oracle.lean`. `make lean` checks that
   the file is current, builds the package (which checks every theorem by
   evaluation in the kernel), and fails on a `sorry` or on any axiom beyond
   `propext`, `Classical.choice` and `Quot.sound`.

### Adding a scenario

1. Write `tools/oracle/<name>.json`: the engine (`budget`, `max_seqs`,
   `block_size`, `num_blocks`, optionally `chunk`) and the `requests`
   (`prompt`, `out`, optionally `arrive`).
2. Run `VLLM_PLUGINS= python tools/vllm_oracle.py tools/oracle/<name>.json`
   and save its output as `tools/oracle/<name>.out.json`. This is the answer
   the theorem will state. The script drives the real scheduler: it needs a
   Python environment with vLLM installed and a full vLLM checkout at the
   pinned revision beside the serQ checkout (`../ref/vllm`, which it imports
   `tests.v1.core.utils` from). The sparse `ref/vllm` that
   `scripts/fetch_vllm_ref.sh --sparse` makes for the citation check is not
   enough.
3. Add the name to the list in `tests/vllm_oracle.rs::scenarios`, run
   `make oracle-ir`, then `make check` and `make lean`. The interpreter and
   the Lean semantics now have to agree. The generator finds the scenarios
   by listing the directory, so it needs no list of its own.

### What fits in the fragment

The Lean semantics runs a fragment of the IR ([IR](ir.md), the Lean
fragment). In practice, a program is in it when:

- one step engine as stage 0 whose `cost` is affine in `tokens`,
  `prefilled` and `decoders` with natural coefficients and a constant term
  of at least 1 (`cost 1` is the step clock),
  serving its residents in admission order (no `serve` clause but the
  default), its `chunk` a constant or an expression of `residents` and
  `queued(p)` (vLLM's `residents + queued(p) > 1 ? c : 0`),
  and any other stage a `delay`;
- the pools are LRU, and either admitted via the engine or the engine's
  memory with `preempt lifo`, with no queue key and no spill;
- nothing is drawn: no `~`, no `poisson` arrivals; the sessions are
  explicit, with preset attributes, and the workload has no `turn` block (a
  trace is inlined with `serq ir --inline-trace`);
- the statements are `turn`, `hold`, `run`, `set`, `observe`, `branch`,
  `loop` and `end`, and the expressions are naturals, attributes, `now`,
  `cachedin`, `budget_left`, `min`, `max`, `+`, `-`, `*`, `floor(a / b)`,
  comparisons and `?:`.

This is why the Lean side has request-level programs such as
`vllm_request.sq`, and not `vllm.sq`: a stochastic workload is outside the
fragment by construction. A program outside it does not get skipped. The
generator stops with `FAIL: outside the Lean fragment: <what>` and writes
nothing, so a new construct that one oracle program uses stops every
theorem until the generator can translate it.

### Changing the language's size

`tools/metrics.json` records the size a reader has to learn: the IR's
variants, the keywords, the functions and the context variables, and the
code lines (over 30 characters, comments stripped, `let`s aside) that three
or more of `examples/` and `lib/` repeat. `make check` fails when it is
not current, so a change that adds a construct shows it in its diff, and a
change that deletes one shows that too. `make metrics` regenerates it and
prints the spec's and the programs' length beside it. A PR that grows the
surface says why; a repeated line in `clones` is a definition waiting to be
written.

### Changing the IR

`IR_VERSION` identifies meaning, not shape ([IR](ir.md), Stability): a field
or variant removed, renamed or retyped, or a change of meaning under the same
shape, bumps it once the version is tagged. The generator pins the version it
reads and refuses any other
(`IR version N (this generator reads M)`). So an IR change moves three
things in one PR: the version, `tools/oracle/` (`make oracle-ir`), and
`scripts/gen_lean_oracle.py` (its pin, and the new node if a committed
program uses one), with `lean/Serq/Oracle.lean` regenerated; `make lean`
fails until all three agree ([IR](ir.md), Stability).
