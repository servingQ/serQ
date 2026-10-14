# Workload

```serq
workload {
  arrive poisson(rate);  |  arrive renewal(gap);  |  arrive closed(n);  |  arrive batch(n);  |  arrive none;
  trace "file.csv" [ordered];
  init block
  turn block
  send (NAME [: expr] [, NAME [: expr]]*);
  [session block]
}
```

How sessions arrive and what each turn brings. `init` and `turn` blocks may
only `set` and `observe`. Random draws in the workload use their own stream.

The workload is a file of its own, given when the program runs:
`serq run model.sq --workload workload.sq` (pyserq:
`compile("model.sq", workload="workload.sq")`). The file has no `fn main()`;
it declares its `use`s, constants and inputs (`let`, `args.number` among
them), definitions (`def`), the stages it runs on itself, the pools it
holds itself (`pool live { cap 24; }`, no `on`) and one `workload { … }`.
A `workload` inside a program's `fn main()` is an error
([paired files](../language.md#paired-files)). A `trace` is read beside
the workload file.

| Clause | Description |
|---|---|
| [`arrive`](#arrive) | Choose the arrival process. Default: `none`. |
| [`trace`](#trace) | Supply session turns from a CSV corpus. |
| [`init`](#init) | Initialize a session once at arrival. |
| [`turn`](#turn) | Set attributes whenever the session executes `turn;`. |
| [`send`](#send) | Bind what each request carries: the server's parameters. |
| [`session`](#session) | Describe how completed turns lead to the next; default: one turn. |
| [`hidden`](#hidden) | Refused here: the server hides a parameter, `server (hidden o)`. |

Workload assignments produce [`Size`](attributes.md#sizes-values-and-costs)
attributes. The server reads only what the workload sends it, and
overwrites none. `init` and `turn`
produce no resource costs. The server interprets request quantities with
[`cost`](functions.md#cost); the session may use costs of client resources.

## `arrive`

| Form | Argument | Sessions arrive |
|---|---|---|
| `poisson(rate)` | `const`, arrivals per clock unit | one at time 0, then at exponential gaps of mean `1/rate` |
| `renewal(gap)` | `expr` of constants and draws | after one gap, then at every further gap; `gap` is evaluated anew for each |
| `closed(n)` | `const`, positive integer | `n` at time 0, and a new one whenever a session ends |
| `batch(n)` | `const`, positive integer | `n` at time 0, and no more |
| `none` | | never (the default) |

`poisson` and `renewal` are *open*: they arrive until the horizon, or until
[`--arrivals N`](program.md#run) has had its `N`.

### `renewal`

```serq
arrive renewal(~h2(2, 4));     // interarrival times of mean 2, CV² 4
arrive renewal(2);             // one every 2 clock units
```

| Argument | Type | Description |
|---|---|---|
| `gap` | `expr` | The interarrival time. It may read numbers, `let` constants, [functions](functions.md#arithmetic) of them and [draws](distributions.md), and not a session attribute, a pool, a stage or a context variable. Each gap must be positive and finite, or the run aborts. |

The first arrival is one gap after time 0, where `poisson` has one at 0: with
the same seed, `renewal(~exp(1 / rate))` arrives at the same times as
`poisson(rate)` after its first arrival, and has no arrival at 0. Gaps draw
from the arrival stream.

## `trace`

```serq
trace "file.csv" [ordered];
```

| Argument | Type | Description |
|---|---|---|
| file | string | Path relative to the program. Columns `session,turn,new,out,think[,forced]`, one row per turn. |
| `ordered` | flag | Replay corpus sessions in order, cycling back to the first when exhausted. Without it, each session samples one corpus session and replays its turns in order. |

At every `turn;` the next turn sets `new`, `out`, `think` and `forced`, and
sets `more` to 1 while another turn remains ([attributes](attributes.md)).

## `init`

```serq
init { set x = expr; … }
```

Runs once, when the session arrives. Moment `Session`.

## `turn`

```serq
turn { set x = expr; … }
```

Runs at every [`turn;`](statements.md#turn) statement of the session.
Moment `Session`.

## `send`

```serq
send (input: K + n, output: o);
send (s);
```

| Argument | Type | Description |
|---|---|---|
| `NAME: expr` | a parameter of the [server](program.md#server), `expr` | The value a request carries for the parameter. |
| `NAME` | a parameter named as a workload attribute | The attribute, as it is. |

What each request carries to the server, `server (input, hidden output) {
… }`: its parameters, which the server, its gateways'
routes and the scheduler's expressions read instead of
the workload's attributes. Each `turn;` evaluates the binding after the
`turn` block's draws, at moment `Session`, with the attributes as they are
then.

Each parameter is named, once; a name the server does not
take is an error. A parameter named as an attribute the workload sets is
that attribute, sent as it is, `send (s)`, and `s: s` is refused as a
second spelling of it. The workload reads the reply after `turn;` as
`reply.R` ([Program](program.md#server)), and no other attribute the server,
the scheduler or an engine sets; a parameter bound to an expression is the
server's name, which the workload does not read. `send ()` is refused: a
workload whose server takes nothing sends nothing.

## `session`

Optional: describes the sequence of turns and the time between them.
Without it, each arriving session makes one turn and finishes. See
[Program](program.md#session). The time between turns, a tool call or a
person reading, is [`tool Z;`](serving.md#tool): with no stage `tool`
declared, it runs on a delay the workload owns (an infinite server, named
`tool` in the report), so the deployment declares nothing for it.

## `hidden`

```serq
server (input, hidden output) { … }
```

| Argument | Type | Description |
|---|---|---|
| `NAME` | a parameter of the server | The scheduler may not read it. |

Hiding is a parameter's: a `hidden` clause in the workload is refused.

Hidden attributes may be read in session expressions and a claim's `given`
condition. They are forbidden in scheduler expressions, including hold
headers, queue and eviction keys, and stage scheduling rules. See
[evaluation moments](context.md#reading-hidden-attributes).

An attribute the scheduler sets (`cached`, `computed`) cannot be hidden,
and a name nothing sets is an error. The [vLLM program](../use-cases/vllm.md)
hides the output length, `server (input, hidden output)`, preventing admission from reserving memory using
future output length.

The engine's request and the `server` are the rest of the scheduler, and
there a hidden attribute is the target's until a run reveals it. A request
may run work by it (`decode (o - 1)`: the model ends the run, not the
scheduler), cache by it at release, and observe it. A run whose work necessarily reads it on every
path reveals it when the run ends, since the end of a decode is the EOS the scheduler sees. After
that the server may decide on it.

Before it is revealed, these are link errors when they read it, or read an
attribute the server set from it (`set long = o > 100;`): an `if` or `while`
condition, a `choose`, a `grow` or `load` amount, or the index that picks a pool or
stage. An attribute set from it is revealed with it. What the workload sets
from it before a request counts as set from it as well, in `init`, in `turn`
(`turn { set long = o > 100; }` hands the server the same unrevealed length)
or in the session before its `turn`. A request's end reveals the turn's
outputs to the workload: the session's statements after it read the turn
that has ended (`set K = K + n + o;`), until it sets a hidden attribute again.

Paths join conservatively: after a branch, the attribute counts as revealed
only if both arms reveal it, and a loop's body may not run at all. Loop-carried
assignments are checked until no new hidden dependencies reach the next pass.
If one branch assigns `x = a` and another `x = b`, running work by `x` does
not establish that both hidden inputs were read. Conditional expressions and
short-circuit operators follow the same rule; an aggregate body is not
assumed to execute. A hold's
header is read at admission, where the attribute itself stays refused even
after a run (the moment rule above). The workload's statements are the
client's and are not subject to the server statement check.

## Examples

A complete program, a deployment and its workload:

```serq title="model.sq"
fn main() {
  stage svc : delay;
  server (duration) {
    run svc (cost(svc, duration));
    observe elapsed = now;
  }
}
```

```serq title="workload.sq"
workload {
  arrive batch(2);
  init { set rounds = 2; }
  turn { set duration = ~uniform(1, 2); }
  send (duration);
  session {
    loop {
      turn;
      set rounds = rounds - 1;
      if (rounds == 0) { end; }
    }
  }
}
```

Save the deployment as `model.sq` and its workload as `workload.sq`, then run:

```sh
serq run model.sq --workload workload.sq --horizon 10 --seed 10
```

## See also

[Distributions](distributions.md), [attributes](attributes.md),
[run settings](program.md#run), [`pyserq.read_trace`](../python/read-trace.md).
