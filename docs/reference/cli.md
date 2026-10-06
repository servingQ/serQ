# CLI reference

```
serq run   FILE [--seed N] [--horizon T] [--warmup T] [--arrivals N] [--instance F] [--set name=expr]...
                    [--def name=expr]... [--trace F] [--json] [--dump DIR]
serq check FILE [--instance F] [--set name=expr]... [--def name=expr]...
serq ir    FILE [--instance F] [--set name=expr]... [--def name=expr]... [--seed N] [--horizon T] [--warmup T]
                    [--arrivals N] [--trace F] [--inline-trace]
serq draw  FILE [--instance F] [--set name=expr]... [--def name=expr]... [--format tikz|svg] [--out PATH]   (experimental)
serq target FILE [--instance F] [--set name=expr]... [--def name=expr]...
serq fmt   [--check] FILE...
serq --version
```

Append `[-- --name value ...]` to `run`, `check`, `ir`, `draw` or `target`
to supply numeric inputs declared by `args.number`. The separator occurs once;
all interpreter options precede it.

`FILE` is program text (`.sq`) or IR (`.json`, as written by `serq ir`).

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
| `fmt` | format one or more `.sq` files in place; comments, blank lines, number spellings, and aligned trailing comments are preserved |
| `--version` (or `-V`) | print `serq X.Y.Z`, the interpreter's version, for the record of a run; `run --json` writes the same as `serq_version` |

## Options

| Flag | Applies to | Meaning |
|---|---|---|
| `--seed N` | run, ir | RNG seed (default 1) |
| `--horizon T` | run, ir | simulated seconds |
| `--warmup T` | run, ir | exclude the first `T` seconds from measured statistics; claims and whole-run counters still include them |
| `--arrivals N` | run, ir | stop after `N` arrivals and drain their sessions ([a finite run](../api/program.md#a-finite-run)); open workloads only |
| `-- --name value` | run, check, ir, draw, target | numeric program inputs declared with `args.number`; `--name=value` also works. Interpreter flags must precede the separator. Rejected on `.json`, whose inputs are already resolved |
| `--set name=expr` | run, check, ir, draw, target | supply an input declared with `args.number` (plain `let` constants and unknown names are errors; the last value wins). Rejected if the constant, directly or through another `let`, sets a queue family's size. **Rejected on `.json`**: an IR's constants are already folded |
| `--instance F` | run, check, ir, draw, target | read an [instance](../api/program.md#instances) from `F`: each of its `let`s supplies a declared input as `--set` would and each option of its `run` block the flag of the same name, applied where the flag stands, so a later `--set` or flag wins over the instance and the instance over an earlier one. Anything else in `F` (a pool, a `def`, a `use`, a second `run`) is an error |
| `--def name=expr` | run, check, ir, draw, target | replace the body of a declared expression [`def`](../api/program.md#def), which then expands at each use as if written so: a distribution, a policy key or a law per class passed in as a parameter (`--def service='~erlang(4, 1)'`). The body may draw and read what the program's body could; the definition keeps its parameters. Unknown names and statement definitions are errors; the last override of a name wins. **Rejected on `.json`**: an IR's definitions are already expanded |
| `--trace F` | run, ir | replace the program's trace corpus |
| `--inline-trace` | ir | turn the trace file into the sessions' turns, as `CArrival::Sessions` data |
| `--json` | run | print the report as JSON |
| `--dump DIR` | run | write `DIR/<name>.csv` per `observe`, columns `time,session,turn,value`, and `DIR/gauge/<name>.csv` per `gauge`, columns `time,value` (its change points) |
| `--format` | draw | `tikz` (default) or `svg` |
| `--out PATH` | draw | write to a file instead of stdout |
| `--check` | fmt | exit with code 1 and list files that need formatting, without writing them |

## The JSON report

`observes` and `gauges` are objects keyed by name; `stages` and `pools` are arrays.

```bash
serq run examples/multi-turn/vllm.sq --instance examples/multi-turn/instances/vllm/default.sq --json | jq '.observes.ttft.mean'
serq run examples/multi-turn/vllm.sq --instance examples/multi-turn/instances/vllm/default.sq --json | jq '.pools[] | select(.name=="kv") | .preemptions'
```

| Path | |
|---|---|
| top level | `serq_version` (the interpreter that ran, `serq --version`), `horizon` (the configured deadline), `end` (when the run ended: `horizon`, or earlier with `--arrivals`), `warmup`, `seed`, `events`, `arrivals`, `ended`, `turns`, `mean_live` |
| `observes.<name>` | `count`, `mean`, `ci`, `cv2`, `p99` |
| `gauges.<name>` | `mean` (time average over `[warmup, end]`), `ci`, `min`, `max` |
| `claims[]` | present when the program has a `claim`: `name`, `kind` (`every_iteration`, `some_iteration`, `at_end`), `result` (`holds`, `fails`, `witnessed`, `not_witnessed`, `not_evaluated`, `out_of_scope`), `checked` (iterations read, or 1 for a claim read at the end), `failures` (of those, the ones that read 0), `first` (the first failure, or a `some` claim's first witness; `null` when none), `note` (the sessions live at the end, or the session that failed `given`; `null` otherwise). The whole run, warm-up included |
| `stages[]` | stage statistics, below |
| `pools[]` | pool statistics, below |

Non-finite numbers are written as `null`. For stages and pools, `name`
identifies the resource and `index` its array member (`null` for a single resource).

### Stage statistics

| Fields | Meaning |
|---|---|
| `mean_number`, `utilization` | time-average jobs present and fraction of time occupied; on a shared stage, utilization is the time-average capacity carried by its flows |
| `completed`, `throughput`, `mean_wait`, `mean_service` | completions, completions per clock unit, mean queue wait and mean service time |
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
`idle_with_work` as `idle: stage …`.

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
| `mean_used`, `mean_cached`, `mean_queue`, `mean_holders` | time-average allocated units, cached units, waiting sessions and holding sessions |
| `mean_wait` | mean admission wait |
| `admissions`, `evicted_entries`, `evicted_units`, `preemptions`, `spills`, `rejected` | admission, eviction, preemption, spill and rejection counters |
| `stuck` | sessions preempted again without progress past their previous preemption |
| `over_cap` | `null`, or `{queue, need}` when that queue's head asks for more than this pool's capacity at the end of the run |

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
by a correction hint. Paths declared in the program are relative to its directory;
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
serq target examples/multi-turn/vllm.sq
```

```json
{"target": "vllm", "engine": "engine",
 "config": {"max_num_batched_tokens": 8192, "max_num_seqs": 16, "block_size": 16,
            "num_gpu_blocks": 10001, "long_prefill_token_threshold": 0,
            "enable_prefix_caching": true}}
```

`num_gpu_blocks` counts vLLM's null block, which the KV pool leaves out. A
cache clause on the KV pool is the prefix cache. The stages' `cost` is not
read, because vLLM runs the model. A program with another policy is
refused, and the error names the construct:

- an `iteration` body other than vLLM's, `iteration { serve; admit while
  (!preempted); }` (a stage without one is vLLM's), and so `serve only` and
  a register;
- `serve exclusive prefill`, or `serve by` keys that read anything but
  `decoding`, `admission`, numbers and arithmetic on them;
- a selection key, an eviction key or a spill on a pool, a `preempt` other
  than `lifo` (the latest admitted, re-queued at the head), or `reserve
  held`;
- a `fifo` or `ps` stage, or a request's legs (`fork`);
- a pool count other than two;
- a constant chunk cap: vLLM lifts the cap for a request alone
  (`scheduler.py:606-616`), so the program writes
  `chunk long_prefill(reqs, c)` from `lib/vllm.sq`, or `chunk 0`.

`serve by` keys that vLLM's scheduler can observe are the programmable
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
