# Program

```
program := (let | def | use)* fn main() { item* }
item    := let | def | use | pool | stage | queue | workload | server | share | gauge | claim
```

`fn main()` is the single execution entry point: it constructs the deployment and
workload. The session body then runs for each arrival.
Top-level constants and definitions do not execute a simulation. Libraries
contain definitions and imports, and cannot declare a `main` or a deployment.
There are no arguments or return value on `main`; use `std/args` for inputs.

Items are read in order and declarations come first: a [serving form](serving.md)
finds its stage among the stages declared above it.

| Declaration | Description |
|---|---|
| `fn main()` | Construct the deployment and workload. |
| [`let`](#let) | Declare a constant. |
| [`args.number`](#stdargs) | Declare a numeric program input. |
| [`def`](#def) | Define a reusable expression or statement body. |
| [`use`](#use) | Import a library relative to a source file. |
| [`pool`](#pool) | Declare resource capacity, cache and admission policy. |
| [`stage`](#stage) | Declare a server or step engine. |
| [`workload`](#workload) | Define arrivals and request attributes. |
| [`session`](#session) | Define the request's sequence of actions. |
| [`server`](#server) | Define the scheduler side of a two-sided program. |
| [`share`](#share) | Select rate sharing for flows across stages. |
| [`gauge`](#gauge) | Measure a function of deployment state over time. |
| [`claim`](#claim) | State a property of the program's paths. |

## `let`

```serq
let NAME = expr;
```

A named constant, folded at link time. A plain `let` is internal to the
program: CLI options and instances cannot replace it.

| Argument | Type | Description |
|---|---|---|
| `NAME` | identifier | May not also be a session attribute: the linker rejects it. |
| `expr` | `const` | May read earlier constants. |

### `std/args`

```serq
use "std/args";

fn main() {
  let rate = args.number("arrival_rate", 0.3);
  stage svc : delay;
  workload {
    arrive poisson(rate);  }
  server {
    run svc (1);
    }
}
```

`args.number("name", default)` declares a numeric input as the entire
initializer of a `let` inside `main`. Its external name is an identifier;
its local binding may have a different name. Read each input once and reuse
that binding. The default is a constant expression and may read earlier
constants. Values are numbers (including `inf`), never NaN. Strings,
positional arguments and implicit access to the host environment are not
part of this numeric library.

`serq run model.sq --horizon 10 -- --arrival_rate 0.5` supplies the value. The separator
keeps the program's options apart from the interpreter's `--seed`,
`--horizon`, and other run settings. `--arrival_rate=0.5` works too;
repeated options use the last value. Unknown names, missing values and
invalid numbers fail before simulation. `--set arrival_rate=0.5`,
instances and Python `sets={"arrival_rate": 0.5}` bind the same declared
input. `--set` and API strings can also supply a constant expression.

The frontend provides `std/args` on every installation; it does not read a
file or the process environment. Inputs become constants before IR is
produced. Runtime expressions cannot call `args.number`. Array sizes are
resolved during parsing, so supplying an input that affects an array size
is refused, including through a derived constant.

### Instances

An instance supplies the model's declared inputs and its execution conditions.
It may contain `let NAME = expr;` for inputs declared with `args.number` and
one `run` configuration block with numeric values. It cannot declare pools,
stages, workloads or definitions. The configuration block belongs only to
an instance; a model's `run STAGE (work);` performs stage work.

```serq
// experiment.sq
let spacing = 3.0;
run { horizon 6000; warmup 0; seed 2; }
```

```sh
serq run examples/replay/vllm_replay.sq --instance experiment.sq
```

This supplies the same inputs and execution conditions as
`--set spacing=3.0 --horizon 6000 --warmup 0 --seed 2`. Flags and instances
apply in command-line order; later values win.

Example commands state their execution conditions directly. An instance is
optional: use one to name an experiment, and select it explicitly with
`--instance`. Settings files are never auto-loaded.

## `def`

```serq
def NAME ( PARAM, … ) { expr }
def NAME ( PARAM, … ) { statement* }
```

Both forms enclose the body in braces. A value definition contains one
expression without a trailing semicolon. A statement definition contains
statements, or is empty. Statements followed by a result expression are
not supported; definitions do not introduce local variables or `return`.

A name for source the program would otherwise repeat. A use, `NAME(arg, …)`
in an expression or `NAME(arg, …);` as a statement, is replaced by the body
with each parameter replaced by its argument, and parsed where it stands: a
serving form in it finds its stage at the use, and a statement body follows
the rules of the block it is used in (no `turn`, `end` or `request` in a
`server`).

An expression definition's body can separately be given from outside: `--def NAME=expr` on the command line, `defs={"NAME": "expr"}` in
pyserq. The program is then the one written with that body, so a
distribution or a key the program leaves open is a parameter of the run
(`def service() { ~exp(1) }`, run with `--def service='~erlang(4, 1)'`).

```serq
def reusable(x, bs) { floor((x - 1) / bs) * bs }
set hitD = min(cachedin(D[j].kv), reusable(prompt, bs));
```

(`lib/vllm.sq`, and its use in `examples/pd-disaggregation/llmd_nixl_pull.sq`)

| Argument | Type | Description |
|---|---|---|
| `NAME` | identifier | Not a keyword, a function, a distribution, a pool, a stage or a `let`, and not defined twice. Defined before its first use; a body uses only the definitions before it, so none reaches itself. |
| `PARAM` | identifier | Not a keyword or a function, and not a name the body assigns or binds (`set p =`, `choose p`, `p =` in a binding). |
| `arg` | `expr` or reference | A reference (`kv`, `kvD[j]`) is put in as written, so it may name a pool or a stage; any other argument is put in inside parentheses. An argument that draws (itself, or through a definition that draws) may be passed only to a parameter the body reads once, and an argument may not read a name the body assigns. |

Definitions do not create local attributes: names other than parameters
refer to the program's names. Arguments cannot read names the body may
assign, including assignments through nested definitions, hold admission,
`turn` or `request`. For a named gateway request, this includes the selected
gateway's routing assignments.

A statement-definition argument also cannot read the clock or live state
(`now`, `used(kv)`), since the body may use it after time passes. Capture
such a value with `set` first and pass the attribute. Errors in an expansion
identify both the body and its use.

## `use`

```serq
use "path";
```

Reads the definitions of a library: `path` is a file of `def`s (and `use`s),
relative to the file the `use` is in, and its definitions are the program's
from here on. A library read once is not read again. An error in a library
is reported in the library, with the uses it was expanded from.

```serq
use "../../lib/vllm.sq";
…
server {
  set t0 = now;
  set prompt = K + n;
  vllm_request(reqs, kv, engine, prompt, o, t0);
  observe response = now - t0;
}
```

(`examples/multi-turn/vllm.sq`; the library is `lib/vllm.sq`.) A program
given as text rather than read from a file cannot `use`. The library's names
are the program's, as for any [`def`](#def): what a statement definition
sets is the session's attribute, and what it observes is the program's
observation, so a library says in its comments which names it takes.

## `pool`

```serq
pool NAME [ '[' N ']' ] { option* }
```

A counted resource: KV memory, request slots, an offload tier. See [Pool](pool.md).

| Argument | Type | Description |
|---|---|---|
| `NAME` | identifier | |
| `N` | integer literal ≥ 1 | Optional. Declares an array of `N` pools, `NAME[0]` … `NAME[N-1]`. A `let` constant is not accepted here. |

## `stage`

```serq
stage NAME [ '[' N ']' ] : kind;
```

A place where time passes. `kind` is one of `fifo`, `ps`, `delay`, `step`;
see [Stage](stage.md). `N` declares an array, as for `pool`.

## `workload`

```serq
workload { item* }
```

How sessions arrive and how their turns evolve. See [Workload](workload.md).
At most one per program (a second is `duplicate workload`).

## `session`

An optional block inside `workload` describing the sequence of turns.
Omit it for a single turn. `turn;` draws attributes and waits for the
server's response; reaching the end of the block ends the session.

```serq
session {
  turn;
  while (more) {
    set K = K + n + o;
    tool (~exp(Z));
    turn;
  }
}
```

This is an excerpt of the [multi-turn program](../use-cases/vllm.md).
Use `end;` only to exit early. Queue entry calls belong in the server.

## `server`

```serq
server block
```

The deployment's handling of each turn. Every workload uses one server;
the server may route to queues, for example `server { gw.route(); }`.
It completes a turn when its block finishes. It cannot draw another turn
or end the client's session (`turn;` and `end;` are refused).

An explicit session must contain a turn. A top-level `session` is an error.
The default session is `session { turn; }`.
See [the two sides](../language.md#the-two-sides).

## `share`

```serq
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

## `gauge`

```serq
gauge NAME = expr;
```

A function of the deployment's state whose time average over
`[warmup, end]` the report gives, with a batch-means 95 % CI and the least
and greatest value held; `--dump` writes its change points
(`gauge/NAME.csv`). See [Gauges](../language.md#gauges).

| Argument | Type | Description |
|---|---|---|
| `NAME` | identifier | Not also an `observe`'s name, nor another gauge's. |
| `expr` | `expr`, at the `Gauge` moment | Pool and stage observables and constants; not an attribute, a draw, `now`, `cachedin`, `work` or `budget_left`. A pool or stage index is a number (`kv[0]`, or the `kv[k]` of `max k in N (…)`). |

## `claim`

```serq
claim NAME [given (expr)]: every iteration of STAGE (expr);
claim NAME [given (expr)]: some iteration of STAGE (expr);
claim NAME [given (expr)]: at end (expr);
```

A property of the program’s paths. Claims do not change execution. The
interpreter checks each claim on the path it runs; the report records the
result, first failure or witness, and whether the claim was evaluated.
A simulation result alone is not a proof over all paths. See
[Claims](../language.md#claims).

| Argument | Type | Description |
|---|---|---|
| `NAME` | identifier | Not another claim's. |
| `given` | `expr`, at the `Given` moment | Read for each session once its `init` has run, from its attributes and the constants. A session that reads 0 puts the claim out of the run's scope. |
| `STAGE` | a `step` stage | One stage, or an array's member by a constant index (`E[0]`). |
| `expr` | `expr`, at the `Iteration` moment | The cost's variables, `demand`, `served`, `arrived`, `now`, and `queue`, `busy`, `used`, `free`, `holders`, `queued` by a number; not an attribute or a draw. |
| `expr` (`at end`) | `expr`, at the `End` moment | Constants, `now` and `total`, `count`, `largest`, `smallest`, `prefix_total` of an `observe`. |

## Execution settings {#run}

Supply execution settings through [CLI flags](../reference/cli.md), an
explicit [instance](#instances), or [`pyserq.compile`](../python/compile.md).
They are not declarations inside `fn main()`. In a model, `run STAGE (work);`
performs work on a stage.

```sh
serq run model.sq --horizon 10 --warmup 0 --seed 1
```

| Field | Type | Default | Description |
|---|---|---|---|
| `horizon` | number | required | End of the simulation, in the program's clock unit. |
| `warmup` | number | `0` | Measured statistics exclude earlier samples; claims still include them. Must be below `horizon`. |
| `seed` | number | `1` | Seed of the random streams. Arrivals, the workload, the session, eviction and trace sampling each draw from their own. |
| `arrivals` | positive integer | none | Stop after exactly this many arrivals and run until their sessions have all ended. Only with an open workload (`poisson` or `renewal`). |

A source model requires an explicit horizon for execution or IR export.
`check`, `draw`, `target` and `fmt` do not require execution settings.
A serialized IR already contains them; flags can replace them when loading it.

### A finite run

For example: `serq run model.sq --horizon 1e5 --arrivals 1000`.

Without `arrivals` the run ends at `horizon`. With it, the run ends when the
`N`-th session has arrived and every session has ended, and `horizon` is the
deadline for both. It is an error if the deadline passes with fewer than `N`
arrivals or with a session still live, and if the run drains at or before
`warmup`, which would leave nothing to measure. The report gives the time the
run ended as `end`, and its rates and time averages are over `end − warmup`.

```serq
workload {
    arrive renewal(~h2(2, 4)); … }
```

## Examples

A complete program:

```serq
fn main() {
  let duration = 2;
  stage svc : fifo;
  workload {
    arrive batch(2);
  }
  server {
    set t0 = now;
    run svc (duration);
    observe response = now - t0;
  }
  gauge jobs = queue(svc);
  claim done : at end (count(response) == 2);
}
```

Save as `model.sq`, then run:

```sh
serq run model.sq --horizon 10 --seed 1
```

## See also

[Pools](pool.md), [stages](stage.md), [workloads](workload.md),
[`pyserq.compile`](../python/compile.md).
