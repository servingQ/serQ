# CLI reference

```
seq-lang run   FILE [--seed N] [--horizon T] [--warmup T] [--set name=expr]...
                    [--trace F] [--json] [--dump DIR]
seq-lang check FILE [--set name=expr]...
seq-lang ir    FILE [--set name=expr]... [--seed N] [--horizon T] [--warmup T]
                    [--trace F] [--inline-trace]
seq-lang draw  FILE [--view deployment|session] [--format tikz|svg] [--out PATH]
                    [--show-set]                                  (experimental)
seq-lang fmt   [--check] FILE...
```

`FILE` is program text (`.seq`) or IR (`.json`, as written by `seq-lang ir`).

## Commands

| | |
|---|---|
| `run` | execute the program as a discrete-event simulation and print the report |
| `check` | parse, resolve every name and fold the constants; print a summary. This is what `make check` runs over every program |
| `ir` | print the program's [IR](../ir.md) as JSON |
| `draw` | render the program as a figure ([visualization](../visualization/index.md)) |
| `fmt` | format one or more `.seq` files in place; comments, blank lines, number spellings, and aligned trailing comments are preserved |

## Options

| Flag | Applies to | Meaning |
|---|---|---|
| `--seed N` | run, ir | RNG seed, overriding the program's `run` block |
| `--horizon T` | run, ir | simulated seconds |
| `--warmup T` | run, ir | seconds discarded before anything is recorded |
| `--set name=expr` | all | override a declared `let` constant (unknown names are errors; the last override of a name wins). **Rejected on `.json`**: an IR's constants are already folded |
| `--trace F` | run, ir | replace the program's trace corpus |
| `--inline-trace` | ir | turn the trace file into the sessions' turns, as `CArrival::Sessions` data |
| `--json` | run | print the report as JSON |
| `--dump DIR` | run | write `DIR/<name>.csv` per `observe`, columns `time,session,turn,value` |
| `--view` | draw | `deployment` (default) or `session` |
| `--format` | draw | `tikz` (default) or `svg` |
| `--out PATH` | draw | write to a file instead of stdout |
| `--check` | fmt | exit with code 1 and list files that need formatting, without writing them |
| `--show-set` | draw | include `set` statements in the session view |

## The JSON report

`observes` is an object keyed by name; `stages` and `pools` are arrays.

```bash
seq-lang run examples/multi-turn/vllm.seq --json | jq '.observes.ttft.mean'
seq-lang run examples/multi-turn/vllm.seq --json | jq '.pools[] | select(.name=="kv") | .preemptions'
```

| Path | |
|---|---|
| `observes.<name>` | `count`, `mean`, `ci`, `cv2`, `p99` |
| `stages[]` | `name`, `mean_number`, `utilization`, `completed`, `throughput`, `mean_wait`, `mean_service`, `iterations` |
| `pools[]` | `name`, `mean_used`, `mean_cached`, `mean_queue`, `mean_holders`, `mean_wait`, `admissions`, `evicted_entries`, `evicted_units`, `preemptions`, `spills`, `rejected`, `stuck` (sessions preempted again without progress since their previous preemption) |

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
integer, `--horizon` a finite positive number, and `--warmup` a finite nonnegative
number. Program loading, validation, and runtime errors exit with code 1. Failed commands
do not write a report to stdout.

Trace CSV errors identify the actual trace path, row, and column name, followed
by a correction hint. Paths declared in the program are relative to its directory;
`--trace` paths are relative to the current directory. `run` and
`ir --inline-trace` use the same trace diagnostics.

Name-resolution errors in `.seq` programs show the line, character column,
source excerpt, and a correction hint. A close, unambiguous name of the same
kind is suggested with its declaration location; duplicate pools and stages
identify both declarations. Locations survive `server`/`request` expansion and
header bindings. `.json` validation errors use IR context instead of inventing
text-source locations. Syntax errors, including unclosed `/*` comments, point
to the offending source location.
