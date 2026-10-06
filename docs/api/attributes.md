# Attributes

A session carries named numeric attributes, initially 0. Names assigned by
[`set`](statements.md#set) or [`choose`](statements.md#choose) in session code
are attributes; a `set` in a stage’s `iteration` body instead assigns a stage
register. The built-in attributes below need no assignment.
A name may not be both an attribute and a `let` constant, and neither may take
a [context variable](context.md)'s name.

## Sizes, values and costs

Attributes have inferred types. Assignments in the workload produce `Size`:
request quantities and client state. The server may read them but cannot
assign them. Server assignments produce ordinary `Value` bookkeeping or a
resource-specific `Cost`. Calculate remaining work or a bounded response in
a separate server attribute; keep the client's original request unchanged.

`cost(resource, expression)` interprets an ordinary quantity as that
resource's cost. `run` requires a stage cost; `hold`, `grow` and `load`
require pool costs. For example:

```serq
workload {
  arrive batch(1);
  turn { set items = 3; }
  session { turn; }
}
server {
  set service = cost(svc, 2 * items);
  hold mem (cost(mem, items)) { run svc (service); }
}
```

Here `svc` and `mem` are declared resources. The same three items cost six
seconds at a FIFO `svc` with this conversion. Changing the coefficient
changes the deployment model without changing the workload.

`Size` is an ownership type, not a physical unit: tokens, bytes, client
continuation flags and thinking intervals are all ordinary quantities.
The type checker cannot infer whether an arbitrary numeric formula was
intended as service time. It requires the server to state the resource
interpretation explicitly. `cached` and `computed` are scheduler-owned
`Value`s; the other built-ins below are `Size`s.

Costs preserve their type through aliases, scalar multiplication or
division, negation, and `abs`, `floor`, `ceil`. Addition, subtraction,
`min`, `max`, comparisons and conditional branches require matching cost
types. A cost cannot be mixed with a bare number or converted to another
resource's cost. Costs must be assigned on every path before use. A cost
attribute used by a hold's `cache` must already be assigned before entering
the hold, since release, rejection and preemption can end it early.

Workload `init` and `turn` cannot construct or read costs. A client's
`session` can use its own resources, such as a tool stage or a
conversation-wide reservation, but cannot create or read costs of stages
run or pools acquired, grown or loaded by the server. Scheduling declarations, gauges and
claims use ordinary quantities; `observe` can record a cost's numeric value.
See [`cost`](functions.md#cost) for resource families and evaluation timing.

## Built-in attributes

| Name | Set by | Meaning |
|---|---|---|
| `serial` | arrival | arrival order, counted from 0 |
| `turn_no` | `turn;` | turns taken so far: 0 before the first `turn;`, 1 after it |
| `cached` | admission | units of the session's own cached prefix consumed at its last admission (`reuse`); 0 after the admission of a hold without `cache`, which consumes none |
| `computed` | preemption | the position the hold had computed when it was preempted; 0 on a first execution and after a hold completes |

With a [`trace`](workload.md#trace), `turn;` also sets:

| Name | Meaning |
|---|---|
| `new` | new tokens this turn brings |
| `out` | output tokens of the turn |
| `think` | the think time the trace records for the turn |
| `more` | 1 while another turn remains, 0 after the last |
| `forced` | 1 if the trace forces the turn to miss the prefix cache (an optional sixth column, else 0) |

`cached` and `computed` are set by the scheduler, so they cannot be
[`hidden`](workload.md#hidden).

A program that resumes after a preemption reads `computed`; one that
recomputes from the prompt alone does not read it.

## Examples

A complete program:

```serq
fn main() {
  stage svc : delay;
  workload {
    arrive batch(2);
  }
  server {
    run svc (cost(svc, 1));
    observe request_id = serial;
    observe current_turn = turn_no;
  }
}
```

Save as `model.sq`, then run:

```sh
serq run model.sq --horizon 10
```

## See also

[Context variables](context.md), [trace workloads](workload.md#trace),
[`set`](statements.md#set).
