# Program

```
program := item*
item    := let | def | use | pool | stage | workload | session | server | share | run
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

## `def`

```seq
def NAME ( PARAM, … ) = expr;
def NAME ( PARAM, … ) { statement* }
```

A name for source the program would otherwise repeat. A use, `NAME(arg, …)`
in an expression or `NAME(arg, …);` as a statement, is replaced by the body
with each parameter replaced by its argument, and parsed where it stands: a
serving form in it finds its stage at the use, and a statement body follows
the rules of the block it is used in (no `turn`, `end` or `request` in a
`server`). The AST and the IR hold the expansion, so a program with a `def`
has the IR of the one written out.

```seq
def reusable(x) = floor((x - 1) / bs) * bs;
set hitmax = reusable(prompt);
```

(`examples/pd-disaggregation/llmd_nixl_pull.seq`)

| Argument | Type | Description |
|---|---|---|
| `NAME` | identifier | Not a keyword, a function, a distribution, a pool, a stage or a `let`, and not defined twice. Defined before its first use; a body uses only the definitions before it, so none reaches itself. |
| `PARAM` | identifier | Not a keyword or a function, and not a name the body assigns or binds (`set p =`, `choose p`, `p =` in a binding). |
| `arg` | `expr` or reference | A reference (`kv`, `kvD[j]`) is put in as written, so it may name a pool or a stage; any other argument is put in inside parentheses. An argument that draws (itself, or through a definition that draws) may be passed only to a parameter the body reads once, and an argument may not read a name the body assigns. |

Only the parameters are the definition's own. Every other name in the body is
the program's: an attribute the body sets is the session's attribute, as it
would be written out. So that a use reads as a call, an argument that reads
a name the body assigns is refused rather than read after the assignment:
a `set`, `choose` or binding of the body, the attributes a hold's admission
sets (`cached`, `computed`) when the body holds, and what a `turn;` or
`request;` in the body assigns, directly or through a definition the body uses. For the same reason an argument of statements may not read the clock or
live state (`now`, `used(kv)`): the body would read it after its runs. Name
the value with `set` first and pass the name. An error in
an expansion is reported in the body, with a note naming the use.

## `use`

```seq
use "path";
```

Reads the definitions of a library: `path` is a file of `def`s (and `use`s),
relative to the file the `use` is in, and its definitions are the program's
from here on. A library read once is not read again. An error in a library
is reported in the library, with the uses it was expanded from.

```seq
use "../../lib/vllm.seq";
…
server {
  set t0 = now;
  set prompt = K + n;
  vllm_request(reqs, kv, engine, prompt, o, t0);
  observe response = now - t0;
}
```

(`examples/multi-turn/vllm.seq`; the library is `lib/vllm.seq`.) A program
given as text rather than read from a file cannot `use`. The library's names
are the program's, as for any [`def`](#def): what a statement definition
sets is the session's attribute, and what it observes is the program's
observation, so a library says in its comments which names it takes.

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
`end`) next to the deployment's (`hold`, `prefill`, …). At top level it is the
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
| Admission is written | `hold … at admission (…)` |

A workload `session` without a `server`, a `server` that is never requested,
and a `server` next to a top-level `session` are errors.

## `share`

```seq
share maxmin;
share bottleneck;
```

How the flows of [runs over several stages](statements.md#run) divide the
stages' capacity. Required when some run holds several stages, an error
otherwise; there is no default.

| Policy | A flow's rate |
|---|---|
| `maxmin` | Max-min fair: every flow's rate rises together until a stage fills; the flows through it stop there, the others go on. Uses all the capacity it can. |
| `bottleneck` | Its equal share at the tightest of its stages, `min over s of φ_s / n_s`. What that leaves at its other stages is unused. |

## `run`

```seq
run { horizon expr; warmup expr; seed expr; arrivals expr; }
```

| Field | Type | Default | Description |
|---|---|---|---|
| `horizon` | `const` | required | End of the simulation, in the program's clock unit. |
| `warmup` | `const` | `0` | Samples before this time are discarded. Must be below `horizon`. |
| `seed` | `const` | `1` | Seed of the random streams. Arrivals, the workload, the session, eviction and trace sampling each draw from their own. |
| `arrivals` | `const`, a positive integer | none | Stop after exactly this many arrivals and run until their sessions have all ended. Only with an open workload (`poisson` or `renewal`). |

`--horizon`, `--warmup`, `--seed` and `--arrivals` override them ([CLI](../reference/cli.md)).

### A finite run

Without `arrivals` the run ends at `horizon`. With it, the run ends when the
`N`-th session has arrived and every session has ended, and `horizon` is the
deadline for both. It is an error if the deadline passes with fewer than `N`
arrivals or with a session still live, and if the run drains at or before
`warmup`, which would leave nothing to measure. The report gives the time the
run ended as `end`, and its rates and time averages are over `end − warmup`.

```seq
workload { arrive renewal(~h2(2, 4)); … }
run { horizon 1e5; warmup 0; arrivals 1000; }
```

!!! note
    The `run` *statement* ([`run STAGE …`](statements.md#run)) and the `run`
    *block* here are unrelated constructs that share a keyword.
