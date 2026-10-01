# CLI reference

```
serq run   FILE [--seed N] [--horizon T] [--warmup T] [--arrivals N] [--set name=expr]...
                    [--def name=expr]... [--trace F] [--json] [--dump DIR]
serq check FILE [--set name=expr]... [--def name=expr]...
serq ir    FILE [--set name=expr]... [--def name=expr]... [--seed N] [--horizon T] [--warmup T]
                    [--arrivals N] [--trace F] [--inline-trace]
serq draw  FILE [--set name=expr]... [--def name=expr]... [--format tikz|svg] [--out PATH]   (experimental)
serq fmt   [--check] FILE...
```

`FILE` is program text (`.sq`) or IR (`.json`, as written by `serq ir`).

## Commands

| | |
|---|---|
| `run` | execute the program as a discrete-event simulation and print the report |
| `check` | parse, resolve every name and fold the constants; print a summary. This is what `make check` runs over every program |
| `ir` | print the program's [IR](../ir.md) as JSON |
| `draw` | render the program as a figure ([visualization](../visualization/index.md)) |
| `fmt` | format one or more `.sq` files in place; comments, blank lines, number spellings, and aligned trailing comments are preserved |

## Options

| Flag | Applies to | Meaning |
|---|---|---|
| `--seed N` | run, ir | RNG seed, overriding the program's `run` block |
| `--horizon T` | run, ir | simulated seconds |
| `--warmup T` | run, ir | seconds discarded before anything is recorded |
| `--arrivals N` | run, ir | stop after `N` arrivals and drain their sessions, overriding the `run` block's `arrivals` ([a finite run](../api/program.md#a-finite-run)); open workloads only |
| `--set name=expr` | all | override a declared `let` constant (unknown names are errors; the last override of a name wins). Rejected if the constant, directly or through another `let`, sets a queue family's size. **Rejected on `.json`**: an IR's constants are already folded |
| `--def name=expr` | all | replace the body of a declared expression [`def`](../api/program.md#def), which then expands at each use as if written so: a distribution, a policy key or a law per class passed in as a parameter (`--def service='~erlang(4, 1)'`). The body may draw and read what the program's body could; the definition keeps its parameters. Unknown names and statement definitions are errors; the last override of a name wins. **Rejected on `.json`**: an IR's definitions are already expanded |
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
| top level | `horizon` (the configured deadline), `end` (when the run ended: `horizon`, or earlier with `--arrivals`), `warmup`, `seed`, `events`, `arrivals`, `ended`, `turns`, `mean_live` |
| `observes.<name>` | `count`, `mean`, `ci`, `cv2`, `p99` |
| `gauges.<name>` | `mean` (time average over `[warmup, end]`), `ci`, `min`, `max` |
| `stages[]` | `name`, `index` (the member's index in a stage array, `null` for a single stage), `mean_number`, `utilization`, `completed`, `throughput`, `mean_wait`, `mean_service`, `iterations`, and for a step stage: `prefill_only`, `decode_only`, `mixed` (fractions of the measured time an iteration of prefill only, decodes only, or both was running; the rest is idle), `mean_decodes` (time-average decodes in the running iteration, 0 while none runs), `mean_decode_batch` and `mean_decode_step` (the decodes and the duration of an iteration that carried a decode, averaged over those started after warm-up: the batch a decode is in and the step it waits for), `mean_itl`, `itl_p50`, `itl_p99` (the gaps between a turn's successive tokens, a session's tokens with the same `turn_no`, that end on the stage after warm-up, wherever the earlier token was: a transfer between a prefill engine's first token and a decode engine's second is in the gap, and so is a preemption; a prefill's end is the next token after a decode or after a preemption on the previous token's stage, otherwise the first, replacing any before it, so a decoder's recompute replaces a prefiller's dropped token; a turn's gaps add up to its last token less its first; the quantiles are within 0.5 %, exact for a single value, the mean exact); 0 for the fractions and `mean_decodes`, `null` for the others, on other stages. An iteration that only preempted counts as idle; `utilization` is the time with a job present, which differs from `1 - idle` while residents stall. `iterations` counts the whole run, warm-up included |
| `pools[]` | `name`, `index` (the member's index in a pool array, `null` for a single pool), `mean_used`, `mean_cached`, `mean_queue`, `mean_holders`, `mean_wait`, `admissions`, `evicted_entries`, `evicted_units`, `preemptions`, `spills`, `rejected`, `stuck` (sessions preempted again without progress since their previous preemption) |

## Make targets

| | |
|---|---|
| `make check` | fmt, clippy, tests, every program links and draws, the oracles agree, IR files current |
| `make oracle-ir` | regenerate `tools/oracle/*.ir.json` |
| `make draw-golden` | regenerate `tests/golden/` |

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
identify both declarations. Locations survive `server`/`request` expansion and
header bindings. `.json` validation errors use IR context instead of inventing
text-source locations. Syntax errors, including unclosed `/*` comments, point
to the offending source location.
