# CLI reference

```
serq run   FILE [--seed N] [--horizon T] [--warmup T] [--arrivals N] [--set name=expr]...
                    [--def name=expr]... [--trace F] [--json] [--dump DIR]
serq check FILE [--set name=expr]... [--def name=expr]...
serq ir    FILE [--set name=expr]... [--def name=expr]... [--seed N] [--horizon T] [--warmup T]
                    [--arrivals N] [--trace F] [--inline-trace]
serq draw  FILE [--set name=expr]... [--def name=expr]... [--format tikz|svg] [--out PATH]   (experimental)
serq fmt   [--check] FILE...
serq --version
```

`FILE` is program text (`.sq`) or IR (`.json`, as written by `serq ir`).

## Commands

| | |
|---|---|
| `run` | execute the program as a discrete-event simulation and print the report |
| `check` | parse, link and validate the program; print a summary |
| `ir` | print the program's [IR](../ir.md) as JSON |
| `draw` | render the program as a figure ([visualization](../visualization/index.md)) |
| `fmt` | format one or more `.sq` files in place; comments, blank lines, number spellings, and aligned trailing comments are preserved |
| `--version` (or `-V`) | print `serq X.Y.Z`, the interpreter's version, for the record of a run; `run --json` writes the same as `serq_version` |

## Options

| Flag | Applies to | Meaning |
|---|---|---|
| `--seed N` | run, ir | RNG seed, overriding the program's `run` block |
| `--horizon T` | run, ir | simulated seconds |
| `--warmup T` | run, ir | exclude the first `T` seconds from measured statistics; claims and whole-run counters still include them |
| `--arrivals N` | run, ir | stop after `N` arrivals and drain their sessions, overriding the `run` block's `arrivals` ([a finite run](../api/program.md#a-finite-run)); open workloads only |
| `--set name=expr` | run, check, ir, draw | override a declared `let` constant (unknown names are errors; the last override of a name wins). Rejected if the constant, directly or through another `let`, sets a queue family's size. **Rejected on `.json`**: an IR's constants are already folded |
| `--def name=expr` | run, check, ir, draw | replace the body of a declared expression [`def`](../api/program.md#def), which then expands at each use as if written so: a distribution, a policy key or a law per class passed in as a parameter (`--def service='~erlang(4, 1)'`). The body may draw and read what the program's body could; the definition keeps its parameters. Unknown names and statement definitions are errors; the last override of a name wins. **Rejected on `.json`**: an IR's definitions are already expanded |
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
serq run examples/multi-turn/vllm.sq --json | jq '.observes.ttft.mean'
serq run examples/multi-turn/vllm.sq --json | jq '.pools[] | select(.name=="kv") | .preemptions'
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
