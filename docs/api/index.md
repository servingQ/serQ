# API Reference

seQ's API is its language: the constructs a program is written with. Each
entry gives the signature, what it does, the type of every argument, what it
evaluates to or changes, and where it may appear. [The language](../language.md)
is the specification and argues the design; this section is the lookup.

## By what you are modelling

Start from the part of a deployment you want to write; each row names the
constructs that say it and a program that uses them.

| To model… | Read | In a program |
|---|---|---|
| a request queue and a server | [`stage`](stage.md) (`fifo`, `ps`, `delay`), [`run`](statements.md#run) | `examples/single-turn/mg1.seq`; [tutorial 1](../tutorial/01-a-queue.md) |
| arrivals, turns and think time | [`arrive`](workload.md#arrive), [`turn`](workload.md#turn), [`session`](program.md#session) | [the workloads case study](../case-study-workloads.md) |
| memory a request holds while it runs | [`pool`](pool.md), [`hold`](statements.md#hold) | [tutorial 2](../tutorial/02-memory.md) |
| what the scheduler reads at admission | [`at admission`](statements.md#hold), [moments](context.md) | `lib/vllm.seq` (`known`, `hit`) |
| a prefix cache across turns | [`cache`](statements.md#hold), [`evict`](pool.md#evict), [`cachedin`](functions.md#pool) | [tutorial 4](../tutorial/04-prefix-cache.md) |
| continuous batching and chunked prefill | [`step`](stage.md#step), [`prefill`, `decode`](serving.md#prefill-decode-tool), `growing` | [the vLLM case study](../case-study-vllm.md); [tutorial 5](../tutorial/05-the-engine.md) |
| preemption and recompute | [`preempt lifo`](pool.md#preempt), `computed` ([attributes](attributes.md)) | `lib/vllm.seq` (`known = computed …`) |
| the scheduler's waiting loop and its budget | [`admit via`](pool.md#admit-via), [`budget_left`](functions.md#step-stage) | `lib/vllm.seq` |
| prefill/decode disaggregation | [`lease`](statements.md#hold), [`release`](statements.md#release), [`load`](statements.md#load), [`transfer … from … to`](serving.md#transfer-from-to) | [the P/D case study](../case-study-pd.md) |
| a routing policy | [`choose`](statements.md#choose), [observables](functions.md#observables) | `examples/multi-turn/routing.seq` |
| an engine shared by several programs | [`def`](program.md#def), [`use`](program.md#use) | `lib/vllm.seq` and the four vLLM programs |
| what to measure | [`observe`](statements.md#observe) | every program; `seq-lang run` reports it |

## By construct

| Page | Contents |
|---|---|
| [Program](program.md) | `let`, `pool`, `stage`, `workload`, `session`, `server`, `run` |
| [Pool](pool.md) | the options of `pool` |
| [Stage](stage.md) | `fifo`, `ps`, `delay`, `step` |
| [Workload](workload.md) | `arrive`, `trace`, `init`, `turn`, `hidden` |
| [Statements](statements.md) | the kernel: `hold`, `run`, `grow`, `branch`, … |
| [Serving vocabulary](serving.md) | `prefill`, `transfer`, `decode`, `tool` |
| [Functions](functions.md) | arithmetic functions and observables |
| [Distributions](distributions.md) | `~exp`, `~det`, `~uniform`, `~erlang`, `~h2`, `~bernoulli` |
| [Context variables](context.md) | `tokens`, `age`, `remaining`, … and the moment each exists at |
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
