# API Reference

seQ's API is its language: the constructs a program is written with. Each
entry gives the signature, what it does, the type of every argument, what it
evaluates to or changes, and where it may appear. [The language](../language.md)
is the specification and argues the design; this section is the lookup.

| Page | Contents |
|---|---|
| [Program](program.md) | `let`, `pool`, `stage`, `workload`, `session`, `server`, `run` |
| [Pool](pool.md) | the options of `pool` |
| [Stage](stage.md) | `fifo`, `ps`, `delay`, `step` |
| [Workload](workload.md) | `arrive`, `trace`, `init`, `turn`, `hidden` |
| [Statements](statements.md) | the kernel: `hold`, `run`, `grow`, `branch`, … |
| [Serving vocabulary](serving.md) | `enter`, `admit if`, `prefill`, `transfer`, `decode`, `tool` |
| [Functions](functions.md) | arithmetic functions and observables |
| [Distributions](distributions.md) | `~exp`, `~det`, `~uniform`, `~erlang`, `~h2`, `~bernoulli` |
| [Context variables](context.md) | `ntok`, `age`, `remaining`, … and the moment each exists at |
| [Attributes](attributes.md) | the built-in session attributes |

## Types

| Type | Meaning |
|---|---|
| `expr` | An expression. Every value is a number (`f64`); comparisons and `!` give 0 or 1, and a non-zero operand is true. `inf` is a number. |
| `const` | An expression that folds at link time: numbers, `let` constants, arithmetic and the [arithmetic functions](functions.md#arithmetic). It may not read a session attribute, a pool or a stage, or draw. |
| `pool` | The name of a `pool`, with an index (`kv[j]`, any `expr`) when it is an array. |
| `stage` | The name of a `stage`, indexed the same way. |
| `block` | `{ stmt* }`. |
| `NAME` | An identifier. What it names (a session attribute, a constant, a pool, a stage) depends on where it is written. |

An `expr` is evaluated at one *moment*, and the [context variables](context.md)
it may read depend on which. The moment of each argument is stated in its entry.

`expr`, `pool` and `stage` are the three argument kinds a call can take. A
function names the kind of every argument, and a pool name where an `expr` is
expected (or the reverse) is a link error.
