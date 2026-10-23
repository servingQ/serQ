# CLI reference

```
serq run   FILE --workload W [--seed N] [--horizon T] [--warmup T] [--arrivals N] [--instance F]
                    [--def name=expr]... [--trace F] [--json] [--dump DIR] [--log FILE] [--name value]...
serq check FILE --workload W [--instance F] [--def name=expr]... [--name value]...
serq ir    FILE --workload W [--instance F] [--def name=expr]... [--seed N] [--horizon T] [--warmup T]
                    [--arrivals N] [--trace F] [--inline-trace] [--name value]...
serq draw  FILE --workload W [--instance F] [--def name=expr]... [--format tikz|svg] [--out PATH]
                    [--name value]...   (experimental)
serq target FILE --workload W [--instance F] [--def name=expr]... [--name value]...
serq fmt   [--check] FILE...
serq --version
```

`--name value` supplies a numeric input the program declares with
[`args.number("name", default)`](../api/std/args.md). Any option that is not one of the
interpreter's below is such an input, in any order among them:
`serq run examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --seed 2 --horizon 3000 --Lambda 0.3`
(`Lambda` is the workload file's input; the inputs of both files are the program's).
An input may not be named like an interpreter option (`seed`, `horizon`, …).

`FILE` is program text (`.sq`) or IR (`.json`, as written by `serq ir`).
Program text is the deployment; its workload is a file of its own, `W`,
which `--workload W` gives and the tool pairs with it
([paired files](../language.md#paired-files)). `run`, `check`, `ir`, `draw`
and `target` need it for a `.sq` file: a server with no workload given is an
error, and so is a `workload { … }` in the program's `fn main()`. The
examples' workloads are in `examples/workloads/`, and `examples/pairs.json`
names each program's:
`serq run examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --horizon 3000`.
A message names the file it is about, at its own line; a trace the
workload reads is read next to `W`. An IR (`.json`) is a whole program
already, workload included, and refuses `--workload`. `fmt` formats either
file alone.

Source models contain no execution settings. `run` and `ir` require
`--horizon T` or an explicit `--instance F` that supplies it. `warmup` defaults
to 0 and `seed` to 1. JSON IR contains all resolved settings already.
`check`, `draw`, `target` and `fmt` inspect the model without requiring a horizon.

## Commands

| | |
|---|---|
| `run` | execute the program as a discrete-event simulation and print the report |
| `check` | parse, link and validate the program; print a summary |
| `ir` | print the program's [IR](../ir.md) as JSON |
| `draw` | render the program as a figure ([visualization](../visualization/index.md)) |
| `target` | synthesise the program for [vLLM's scheduler](#the-vllm-target): print the configuration that runs it, or refuse with the construct vLLM cannot run (exit 1) |
| `fmt` | format one or more `.sq` files in place; comments, number spellings, and aligned trailing comments are preserved; a run of blank lines becomes one, and none is kept at the start or end of the file or of a block (inside a block comment they are kept) |
| `--version` (or `-V`) | print `serq X.Y.Z`, the interpreter's version, for the record of a run; `run --json` writes the same as `serq_version` |

## Options

| Flag | Applies to | Meaning |
|---|---|---|
| `--workload W` | run, check, ir, draw, target | the workload file the `.sq` deployment `FILE` runs ([paired files](../language.md#paired-files)); required for a `.sq` file, refused on `.json` |
| `--seed N` | run, ir | RNG seed (default 1) |
| `--horizon T` | run, ir | simulated seconds |
| `--warmup T` | run, ir | exclude the first `T` seconds from measured statistics; claims and whole-run counters still include them |
| `--arrivals N` | run, ir | stop after `N` arrivals and drain their sessions ([a finite run](../api/program.md#a-finite-run)); open workloads only |
| `--name value` | run, check, ir, draw, target | supply a number to an input declared with `args.number`; `--name=value` also works (plain `let` constants and unknown names are errors; the last value wins). Rejected if the constant, directly or through another `let`, sets a queue family's size. **Rejected on `.json`**: an IR's constants are already folded |
| `--instance F` | run, check, ir, draw, target | read an [instance](../api/program.md#instances) from `F`: each of its `let`s supplies a declared input as `--name value` would and each option of its `run` block the flag of the same name, applied where the flag stands, so a later input or flag wins over the instance and the instance over an earlier one. Anything else in `F` (a pool, a `def`, a `use`, a second `run`) is an error |
| `--def name=expr` | run, check, ir, draw, target | replace the body of a declared expression [`def`](../api/program.md#def), which then expands at each use as if written so: a distribution, a policy key or a law per class passed in as a parameter (`--def service='~erlang(4, 1)'`). The body may draw and read what the program's body could; the definition keeps its parameters. Unknown names and statement definitions are errors; the last override of a name wins. **Rejected on `.json`**: an IR's definitions are already expanded |
| `--trace F` | run, ir | replace the program's trace corpus |
| `--inline-trace` | ir | turn the trace file into the sessions' turns, as `CArrival::Sessions` data |
| `--json` | run | print the report as JSON |
| `--dump DIR` | run | write `DIR/<name>.csv` per `observe`, columns `time,session,turn,value`, `DIR/client/<name>.csv` per client metric, the same columns, and `DIR/gauge/<name>.csv` per `gauge`, columns `time,value` (its change points) |
| `--log FILE` | run | write the [run log](log.md) to `FILE` as JSON: what each session did, when, for a replay of the run; the report's JSON then carries it too, as `log` |
| `--format` | draw | `tikz` (default) or `svg` |
| `--out PATH` | draw | write to a file instead of stdout |
| `--check` | fmt | exit with code 1 and list files that need formatting, without writing them |

## The JSON report

`observes`, `client`, `status`, `engines` and `gauges` are objects keyed by name; `stages` and `pools` are arrays.

```bash
serq run examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --horizon 2000 --warmup 200 --seed 1 --json | jq '.client.ttft.mean'
serq run examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --horizon 2000 --warmup 200 --seed 1 --json | jq '.pools[] | select(.name=="kv") | .preemptions'
```

| Path | |
|---|---|
| top level | `serq_version` (the interpreter that ran, `serq --version`), `horizon` (the configured deadline), `end` (when the run ended: `horizon`, or earlier with `--arrivals`), `warmup`, `seed`, `events`, `arrivals` (sessions that arrived, warm-up included), `ended` (sessions that ended after warm-up), `turns` (`turn;` statements run after warm-up: each is one request to the `server`), `mean_live` (time-average sessions alive) |
| `observes.<name>` | `count`, `mean`, `ci`, `cv2`, `p99` |
| `client.ttft`, `client.tpot`, `client.response` | the same fields, of what each turn's client saw, in every program ([what the client sees](../language.md#what-the-client-sees)): `ttft` from the turn to its first output token, `tpot` the time per output token after the first, `response` from the turn to its answer. A turn that commits no token has no `ttft`, and one of a single output token no `tpot` |
| `status.<status>` | every [engine request status](#request-statuses), visited or not: `stays` (stays in it that ended after warm-up), `mean`, `p50`, `p99` (their durations; `null` without any), `number` (time-average engine requests in it over `[warmup, end]`) |
| `engines.<name>` | one per engine that takes requests, in declaration order (`{}` without one): `admissions`, `readmissions`, `token_hit` ([engine statistics](#engine-statistics)) |
| `gauges.<name>` | `mean` (time average over `[warmup, end]`), `ci`, `min`, `max` |
| `claims[]` | present when the program has a `claim`: `name`, `kind` (`every_iteration`, `some_iteration`, `at_end`), `result` (`holds`, `fails`, `witnessed`, `not_witnessed`, `not_evaluated`, `out_of_scope`), `checked` (iterations read, or 1 for a claim read at the end), `failures` (of those, the ones that read 0), `first` (the first failure, or a `some` claim's first witness; `null` when none), `note` (the sessions live at the end, or the session that failed `given`; `null` otherwise). The whole run, warm-up included |
| `stages[]` | stage statistics, below |
| `pools[]` | pool statistics, below |
| `log` | present with `--log`: the [run log](log.md), the same object the file holds (the API's `Report.log`; `--log` with `--json` prints it twice) |

Non-finite numbers are written as `null`, and as `NaN` in the text report:
a mean with no sample, such as the `mean_wait` of a pool no hold waited at
first, is one. For stages and pools, `name`
identifies the resource and `index` its array member (`null` for a single resource).

### Request statuses

A request is one call of an engine, `E (input, output)`, as vLLM's
`Request` is one request to one engine (a P/D turn is two: the
prefiller's and the decoder's). It has exactly one status at any time
([the design](../design/request-state.md)); a program does not read it.

| Status | The request | vLLM |
|---|---|---|
| `waiting` | is in the engine's waiting queue, not scheduled; after a load, with its blocks, waiting for a running slot | `WAITING` |
| `waiting_for_kv` | is admitted to its blocks and not schedulable until its connector's load is done (from an L2, or a KV relation's source, whose send it waits for) | `WAITING_FOR_REMOTE_KVS` |
| `running` | is a resident of the engine's batch, in prefill or decode (its mode); a resident stalled for room to grow is one | `RUNNING` |
| `preempted` | is in the waiting queue again after a preemption, until it runs | `PREEMPTED` |
| `finished` | has generated its output; counted while it keeps its blocks for a save to an L2 or a send to a reader | `FINISHED_*`, with `delay_free_blocks` |

Its KV moves are transfers beside the status, each with its own time, in
the [run log](log.md): a `load` while `waiting_for_kv`, a `save` and a
`send` while `finished`. A change of mode, prefill to decode, is no new
stay. The text report prints the statuses some request was in, after the
client's table; a program that calls no engine prints none:

```text
status          stays    mean     p50     p99  number
--------------  -----  ------  ------  ------  ------
waiting           130  0.0000  0.0000  0.0001   0.000
waiting_for_kv     36  0.0121  0.0114  0.0261   0.015
running            94  0.0359  0.0290  0.2108   0.112
finished           36  0.0121  0.0114  0.0261   0.015
```

(`llmd_nixl_pull.sq` with `chat_reply.sq`, `--horizon 30`.) The 36
`finished` stays are the prefiller's requests whose blocks waited for the
decoder's read; the decoders' requests waited for that KV as long. The
program's own `observe send_held` has the 36 too: it is taken after the
decoder's call returns, so a decoder still at work at the horizon would
leave one out.
A request admitted at once still waited, for no time: the `waiting` median
of 0. `running`'s `number` is the engines' residents, their stages'
`number`.

`stays` counts the stays that ended after warm-up, and `number` is a time
average over the same span, so `number ≈ (stays / span) × mean` (Little's
law) only near a steady state: a run that ends with long stays still open
leaves them out of `stays` but not of `number` (`fastertransformer.sq` at
`--horizon 60` is off by 99 %).

### Engine statistics

One row per engine that takes requests: the sums of what it records of
each admission after warm-up, `E.reused` and `E.prefill`
([the design](../design/engine-admissions.md)). A family's members are one
row under its name, as its records are one per family.

| Field | Meaning |
|---|---|
| `admissions` | admissions after warm-up, the count of `E.reused`; a request preempted and admitted again is admitted again |
| `readmissions` | of those, the admissions of a request preempted since its call |
| `token_hit` | `Σ E.reused / Σ request.known`, each read at an admission: the share of the admitted tokens the engine's cache gave; `null` (`NaN` in the text) when no admission had a token |

The text report prints the table after the statuses:

```text
engine  admits  readmits  token hit
------  ------  --------  ---------
E         1862        10      0.795
```

(`thunderagent.sq` with `closed_agents.sq`, `--horizon 3000 --warmup 300
--N 96 --S 2048 --tr 1`.) The 10 re-admissions end the 10 stays in
`preempted` above it. The counts span the observations' time, after
warm-up: the `E.kv` pools' `admissions` count the whole run, 1130 and 1042
here.

Without a connector an admission's tokens are what it read and what it
computes, and `token_hit` is `Σ E.reused / (Σ E.reused + Σ E.prefill)`. A
token a connector loaded (`E.loaded`) is among the tokens, not the hits:
the share is of what the engine's own cache gave, as vLLM's local counter
is hits over `request.num_tokens` (`kv_cache_manager.py:253-262`), and
vLLM counts the connector's hits apart, over the tokens its local cache
did not give (`scheduler.py:1009-1012`). `vllm_lmcache.sq` with `chat.sq`
at `--horizon 2000` reads 0.394, where the records'
`reused / (reused + prefill)` would read 0.80.

vLLM's `prefix_cache_hits` and `prefix_cache_queries` count the same
admissions less the re-admissions. The scheduler records each admission
(`scheduler.py:1257-1261`), and one of a request with `num_preemptions > 0`
goes to `preempted_hits` and `preempted_queries` instead
(`metrics/stats.py:132-143`), which the Prometheus counters do not read
(`metrics/loggers.py:1052-1057`). `readmissions` is the number of
admissions that difference covers.

### Stage statistics

| Fields | Meaning |
|---|---|
| `mean_number`, `utilization` | time-average jobs present and fraction of time occupied; on a shared stage, utilization is the time-average capacity carried by its flows |
| `completed`, `throughput`, `mean_wait`, `mean_service` | runs completed after warm-up, runs per clock unit, mean wait from a run's arrival at the stage to its start, and mean time from its start to its end. They count runs, not requests: on an engine a request that runs `prefill` then `decode` is two runs (a run of no work is not one), a run's service is its stay among the running requests, and its wait is 0, the wait for admission being the pool's `mean_wait` |
| `iterations` | step iterations over the whole run, including warm-up |
| `prefill_only`, `decode_only`, `mixed` | fractions of measured time running each kind of step iteration; the remainder is idle |
| `mean_decodes` | time-average decodes in the running iteration, 0 while none runs |
| `mean_decode_batch`, `mean_decode_step` | mean decode count and duration of iterations carrying a decode, over those started after warm-up |
| `mean_itl`, `itl_p50`, `itl_p99` | mean, median and 99th percentile of inter-token gaps ending on this stage after warm-up |
| `idle_with_work` | the step stage ended with residents or a waiting queue it serves, and its last iteration attempt scheduled nothing |

On non-step stages, `iterations`, the time fractions and `mean_decodes` are
0; decode-batch, decode-step and inter-token statistics are `null`.
An iteration that only preempts counts as idle. Step utilization measures
time with a job present, so stalled residents can make it differ from the
sum of the three active-time fractions. The text report marks
`idle_with_work` as `idle: engine …`.

Inter-token gaps follow one session and `turn_no`, even across stages:
transfer time and preemption delays count. A prefill's end is the next token
after a decode, or after a preemption on the previous token's stage;
otherwise it starts a new sequence, replacing any earlier first token.
Thus a decoder's recompute replaces a prefiller's dropped token. A turn's
gaps sum to its last-token time minus its first-token time. The mean is
exact; quantiles are within 0.5 %, and exact for a single value.

### Pool statistics

| Fields | Meaning |
|---|---|
| `mean_used`, `mean_cached`, `mean_queue`, `mean_holders` | time-average allocated units, cached units, waiting holds, and sessions holding it (one holding it twice, by a nested hold or a lease, is one; a hold of two pools counts in both) |
| `mean_wait` | mean wait from a hold joining this pool's queue to its admission, after warm-up; a hold waits in the queue of its first pool, so a later pool of the hold has none |
| `admissions`, `evicted_entries`, `evicted_units`, `preemptions`, `spills`, `rejected` | admission, eviction, preemption, spill and rejection counters over the whole run, warm-up included |
| `stuck` | sessions preempted again without progress past their previous preemption |
| `over_cap` | `null`, or `{queue, need}` when that queue's head asks for more than this pool's capacity at the end of the run |
| `growing_at_end` | holds waiting to grow in this pool when the run ends; the text report prints `grow:` |
| `growing_stalled` | every session with an allocation in this pool is waiting to grow in it, so none will free room |

A hold whose units or `reserve` depend on deployment state is not rejected
when it joins the queue. If its demand remains above capacity at the end,
`over_cap` records it and the text report prints `over:`.

## Input errors

`fmt` parses every input before writing any file. It leaves the batch untouched
if one file has a syntax error. `make check` runs `fmt --check` on the example
and tutorial programs.

Commands and their supported options are checked before the program is opened.
An unknown command or option, a missing value, or an invalid value prints the
problem and a correction hint to stderr and exits with code 2. Options belonging
to another command are rejected rather than ignored. `--seed` takes an unsigned
integer, `--horizon` a finite positive number, `--warmup` a finite nonnegative
number, and `--arrivals` a positive integer. Program loading, validation, and runtime errors exit with code 1. Failed commands
do not write a report to stdout.

Trace CSV errors identify the actual trace path, row, and column name, followed
by a correction hint. Paths declared in the workload file are relative to its directory;
`--trace` paths are relative to the current directory. `run` and
`ir --inline-trace` use the same trace diagnostics.

Name-resolution errors in `.sq` programs show the line, character column,
source excerpt, and a correction hint. A close, unambiguous name of the same
kind is suggested with its declaration location; duplicate pools and stages
identify both declarations. `.json` validation errors identify the IR context. Syntax errors, including unclosed `/*` comments, point
to the offending source location.

## The vLLM target

`serq target` compiles a supported program to a vLLM v1 scheduler configuration.
Its configuration can change values, but its policy is fixed. One step
engine serves its running requests in admission order. The request slots
are capped. The KV blocks have an LRU prefix cache and LIFO preemption, and
admission is first come, first served. A program on that architecture
compiles to the configuration that makes vLLM run it:

```bash
serq target examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq
```

```json
{"target": "vllm", "engine": "vllm",
 "config": {"max_num_batched_tokens": 8192, "max_num_seqs": 16, "block_size": 16,
            "num_gpu_blocks": 10001, "long_prefill_token_threshold": 0,
            "enable_prefix_caching": true}}
```

`num_gpu_blocks` counts vLLM's null block, which the KV pool leaves out. A
cache clause on the KV pool is the prefix cache. The stages' `cost` is not
read, because vLLM runs the model. A program with another policy is
refused, and the error names the construct:

- a schedule other than vLLM's, `advance running [order]; admit waiting
  while (running.preempted == 0);` with its gate spelt so ([Engines on
  devices](../language.md#engines-on-devices)), and so an `only` and a
  register;
- `exclusive prefill`, or `advance running by` keys that read anything but
  `decoding`, `admission`, numbers and arithmetic on them;
- a selection key, an eviction key or a spill on a pool, a `preempt` other
  than `lifo` (the latest admitted, re-queued at the head), or `reserve
  held`;
- an engine count other than one, a `fifo` or `ps` stage, or a request's
  legs (`fork`);
- a pool count other than two;
- a constant chunk cap: vLLM lifts the cap for a request alone
  (`scheduler.py:606-616`), so an engine writes `each at most (threshold)`
  with `let threshold = running.count + waiting.count > 1 ? c : inf;`, or
  no `each at most` for no cap;
- a computed chunk cap other than vLLM's adaptive threshold, `let n =
  running.count + waiting.count;` and `let threshold = n > 1 ? max(c,
  floor(B / n)) : inf;` with `B` the engine's `tokens cap`
  (`scheduler.py:617-622`). That one is taken: the configuration has
  `"long_prefill_token_threshold": c` and
  `"long_prefill_token_threshold_adaptive": true`
  (`config/scheduler.py:87-91`).

`advance running by` keys that vLLM's scheduler can observe are the programmable
part. The configuration then also names `scheduler_cls:
serq_vllm.SerqScheduler` (`tools/serq_vllm.py`), and `serve_by` carries
the keys as IR. That scheduler is vLLM's own, except that each step's
running loop visits requests in the program's order. The preemption victim
is still the latest admission, so vLLM picks it apart from the visiting
order. `tests/vllm_target_oracle.rs` compares it with the interpreter on
the scenarios of `tools/oracle/serve/`, once their oracle answers have been
recorded on a vLLM host.

`tests/target.rs` checks the result against the oracle. Each oracle
scenario's program compiles to the configuration that the oracle drove the
real scheduler with, and `tests/vllm_oracle.rs` checks that the scheduler,
so configured, decides as the program does.
