# Attributes

A session carries named numeric attributes. Every name assigned by
[`set`](statements.md#set) or [`choose`](statements.md#choose) is an attribute
of every session, starting at 0. The built-in ones below need no assignment.
A name may not be both an attribute and a `let` constant.

| Name | Set by | Meaning |
|---|---|---|
| `serial` | arrival | arrival order, counted from 0 |
| `turn_no` | `turn;` | turns taken so far: 0 before the first `turn;`, 1 after it |
| `cached` | admission | units of the session's own cached prefix consumed at its last admission (`reuse`) |
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
