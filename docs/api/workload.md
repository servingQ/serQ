# Workload

```serq
workload {
  arrive poisson(rate);  |  arrive renewal(gap);  |  arrive closed(n);  |  arrive batch(n);  |  arrive none;
  trace "file.csv" [ordered];
  init block
  turn block
  session block
  hidden NAME [, NAME]*;
}
```

How sessions arrive and what each turn brings. `init` and `turn` blocks may
only `set` and `observe`. Random draws in the workload use their own stream.

| Clause | Description |
|---|---|
| [`arrive`](#arrive) | Choose the arrival process. Default: `none`. |
| [`trace`](#trace) | Supply session turns from a CSV corpus. |
| [`init`](#init) | Initialize a session once at arrival. |
| [`turn`](#turn) | Set attributes whenever the session executes `turn;`. |
| [`session`](#session) | Define the client side of a two-sided program. |
| [`hidden`](#hidden) | Restrict scheduler access to future attributes. |

Workload assignments produce [`Size`](attributes.md#sizes-values-and-costs)
attributes that a server can read but cannot overwrite. `init` and `turn`
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

## `session`

The session's side of a [two-sided program](program.md#server). See
[Program](program.md#session).

## `hidden`

```serq
hidden o;
hidden o, think;
```

| Argument | Type | Description |
|---|---|---|
| `NAME` | session attribute | The scheduler may not read it. |

Hidden attributes may be read in session expressions and a claim's `given`
condition. They are forbidden in scheduler expressions, including hold
headers, queue and eviction keys, and stage scheduling rules. See
[evaluation moments](context.md#reading-hidden-attributes).

An attribute the scheduler sets (`cached`, `computed`) cannot be hidden,
and a name nothing sets is an error. The [vLLM program](../use-cases/vllm.md)
hides output length `o`, preventing admission from reserving memory using
future output length.

The `server`'s own statements are the rest of the scheduler, and there a
hidden attribute is the target's until a run reveals it. The server may run work by it
(`decode (o - 1)`: the model ends the run, not the scheduler), cache by it
at release, and observe it. A run whose work reads it reveals it when the
run ends, since the end of a decode is the EOS the scheduler sees. After
that the server may decide on it.

Before it is revealed, these are link errors when they read it, or read an
attribute the server set from it (`set long = o > 100;`): a `branch`, a
`choose`, a `grow` or `load` amount, or the index that picks a pool or
stage. An attribute set from it is revealed with it.

Paths join conservatively: after a branch, the attribute counts as revealed
only if both arms reveal it, and a loop's body may not run at all. A hold's
header is read at admission, where the attribute itself stays refused even
after a run (the moment rule above). The workload's statements are the
client's and are not subject to the server statement check.

## Examples

A complete program:

```serq
fn main() {
  stage svc : delay;
  workload {
    arrive batch(2);
    init { set rounds = 2; }
    turn { set duration = ~uniform(1, 2); }
    session {
      loop {
        turn;
        request;
        set rounds = rounds - 1;
        branch (rounds == 0) { end; }
      }
    }
  }
  server {
    run svc (cost(svc, duration));
    observe elapsed = now;
  }
}
```

Save as `model.sq`, then run:

```sh
serq run model.sq --horizon 10 --seed 10
```

## See also

[Distributions](distributions.md), [attributes](attributes.md),
[run settings](program.md#run), [`pyserq.read_trace`](../python/read-trace.md).
