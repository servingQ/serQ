# Program

```
program := item*
item    := let | pool | stage | workload | session | server | run
```

Items are read in order and declarations come first: a [serving form](serving.md)
finds its stage among the stages declared above it.

## `let`

```seq
let NAME = expr;
```

A named constant, folded at link time and overridable from the command line
(`--set NAME=value`; see the [CLI reference](../reference/cli.md)).

| Argument | Type | Description |
|---|---|---|
| `NAME` | identifier | May not also be a session attribute: the linker rejects it. |
| `expr` | `const` | May read earlier constants. |

## `pool`

```seq
pool NAME [ '[' N ']' ] { option* }
```

A counted resource: KV memory, request slots, an offload tier. See [Pool](pool.md).

| Argument | Type | Description |
|---|---|---|
| `NAME` | identifier | |
| `N` | integer literal ≥ 1 | Optional. Declares an array of `N` pools, `NAME[0]` … `NAME[N-1]`. A `let` constant is not accepted here. |

## `stage`

```seq
stage NAME [ '[' N ']' ] : kind;
```

A place where time passes. `kind` is one of `fifo`, `ps`, `delay`, `step`;
see [Stage](stage.md). `N` declares an array, as for `pool`.

## `workload`

```seq
workload { item* }
```

How sessions arrive and how their turns evolve. See [Workload](workload.md).
At most one per program (a second is `duplicate workload`).

## `session`

```seq
session block
```

What every session does, written as one block: the client's side (`turn`,
`end`) next to the deployment's (`enter`, `prefill`, …). At top level it is the
kernel form; inside a `workload` it is the session's side of a
[two-sided program](../language.md#the-two-sides) and says `request;` where the
server runs.

| | |
|---|---|
| Statements allowed | any [statement](statements.md); `request;` only inside `workload` |
| Moment | `Session` |

## `server`

```seq
server block
```

The deployment's side of a request, run at every `request;` of the workload's
`session`. The parser splices the block in place of `request;`, so the IR is
that of the one-block `session`.

| | |
|---|---|
| Refused in a `server` | `turn`, `end`, `request` |
| Admission is written | `admit if … fit where …` |

A workload `session` without a `server`, a `server` that is never requested,
and a `server` next to a top-level `session` are errors.

## `run`

```seq
run { horizon expr; warmup expr; seed expr; }
```

| Field | Type | Default | Description |
|---|---|---|---|
| `horizon` | `const` | required | End of the simulation, in the program's clock unit. |
| `warmup` | `const` | `0` | Samples before this time are discarded. Must be below `horizon`. |
| `seed` | `const` | `1` | Seed of the random streams. Arrivals, the workload, the session, eviction and trace sampling each draw from their own. |

`--horizon`, `--warmup` and `--seed` override them ([CLI](../reference/cli.md)).

!!! note
    The `run` *statement* ([`run STAGE …`](statements.md#run)) and the `run`
    *block* here are unrelated constructs that share a keyword.
