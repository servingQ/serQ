# API Reference

Look up syntax, argument types and evaluation rules here. For the complete
operational semantics, see [The language](../language.md).

## API index {#by-construct}

| Page | Contents |
|---|---|
| [Program](program.md) | `let`, `def`, `use`, `pool`, `stage`, `workload`, `session`, `server`, `gauge`, `claim` |
| [Pool](pool.md) | the options of `pool` |
| [Stage](stage.md) | `fifo`, `ps`, `delay`, `step` |
| [Workload](workload.md) | `arrive`, `trace`, `init`, `turn`, `hidden` |
| [Statements](statements.md) | the kernel: `hold`, `run`, `grow`, `branch`, … |
| [Serving vocabulary](serving.md) | `prefill`, `transfer`, `decode`, `tool` |
| [Functions](functions.md) | arithmetic functions and observables |
| [Distributions](distributions.md) | `~exp`, `~det`, `~uniform`, `~erlang`, `~h2`, `~bernoulli` |
| [Context variables](context.md) | `tokens`, `age`, `remaining`, … and the moment each exists at |
| [Attributes](attributes.md) | the built-in session attributes |

Each entry gives its syntax, argument types, defaults and evaluation rules.
Functions and distributions also state their return value. The **Examples**
sections include complete runnable programs; syntax blocks with placeholders
describe a form.

For the Python package, see the [Python Reference](../python.md). For
commands and report fields, see the [CLI Reference](../reference/cli.md).

## Types

| Type | Meaning |
|---|---|
| `expr` | An expression. Every value is a number (`f64`); comparisons and `!` give 0 or 1, and a non-zero operand is true. `inf` is a number. |
| `const` | An expression that folds at link time: numbers, `let` constants, arithmetic and the [arithmetic functions](functions.md#arithmetic). It may not read a session attribute, a pool or a stage, or draw. |
| `pool` | The name of a `pool`, with an index (`kv[j]`, any `expr`) when it is an array. |
| `stage` | The name of a `stage`, indexed the same way. |
| `observe` | The name of an observation, used by end-of-run aggregates. |
| `block` | `{ stmt* }`. |
| `NAME` | An identifier. What it names (a session attribute, a constant, a pool, a stage) depends on where it is written. |

An `expr` is evaluated at one *moment*, and the [context variables](context.md)
it may read depend on which. The moment of each argument is stated in its entry.

A function signature names each argument's kind: `expr`, `pool` or `stage`,
or `observe` for an observation aggregate. Passing a pool name where an
`expr` is expected (or the reverse) is a link error.

## By what you are modelling

Start from the part of a deployment you want to write; each row names the
constructs that say it and a program that uses them.

| To model… | Read | In a program |
|---|---|---|
| a request queue and a server | [`stage`](stage.md) (`fifo`, `ps`, `delay`), [`run`](statements.md#run) | `examples/single-turn/mg1.sq`; [tutorial 1](../tutorial/01-a-queue.md) |
| arrivals, turns and think time | [`arrive`](workload.md#arrive), [`turn`](workload.md#turn), [`session`](program.md#session); think time as a [`delay`](stage.md#delay) stage, [`tool`](serving.md#prefill-decode-tool) | [the workloads use case](../use-cases/workloads.md) |
| replaying a production trace | [`trace`](workload.md#trace) | `examples/replay/vllm_replay.sq` ([measured replay](../use-cases/vllm.md#measured-replay)) |
| memory a request holds while it runs | [`pool`](pool.md), [`hold`](statements.md#hold) | [tutorial 2](../tutorial/02-memory.md) |
| what the scheduler reads at admission | [`at admission`](statements.md#hold), [moments](context.md) | `lib/vllm.sq` (`known`, `hit`) |
| a prefix cache across turns | [`cache`](statements.md#hold), [`evict`](pool.md#evict), `cached` ([attributes](attributes.md)), [`drop`](statements.md#drop); [`cachedin`](functions.md#pool) for the lookup at admission | [tutorial 4](../tutorial/04-prefix-cache.md); `lib/vllm.sq` for `cachedin` |
| an offload tier for evicted prefixes | [`spill`](pool.md#spill) | `tests/pool_semantics.rs` |
| continuous batching and chunked prefill | [`step`](stage.md#step) (`budget`, `chunk`, `cost`), [`prefill`, `decode`](serving.md#prefill-decode-tool), [`growing`](statements.md#run) | [the vLLM use case](../use-cases/vllm.md); [tutorial 5](../tutorial/05-the-engine.md) |
| the order requests are admitted and served in | [`queue by`](pool.md#queue), [`serve`](stage.md#serve) (`serve decode first`, `serve by (…)`) | `examples/multi-turn/replica.sq` (`serve decode first`); `examples/papers/kong_svf.sq` (`queue by`) |
| preemption and recompute | [`preempt lifo`](pool.md#preempt), `computed` ([attributes](attributes.md)) | `lib/vllm.sq` (`known = computed …`) |
| the scheduler's waiting loop and its budget | [`admit via`](pool.md#admit-via), [`budget_left`](functions.md#step-stage) | `examples/replay/vllm_replay.sq` (`admit via engine`), [tutorial 5](../tutorial/05-the-engine.md); `lib/vllm.sq` (`budget_left`) |
| what the scheduler may not read | [`hidden`](workload.md#hidden) | every vLLM program hides the output length (`o`; `out` in the replay) |
| prefill/decode disaggregation | [`lease`](statements.md#hold), [`release`](statements.md#release), [`load`](statements.md#load), [`transfer … from … to`](serving.md#transfer-from-to) | [the P/D use case](../use-cases/pd.md) |
| a routing policy | [`choose`](statements.md#choose), [observables](functions.md#observables) | `examples/multi-turn/routing.sq` |
| an engine shared by several programs | [`def`](program.md#def), [`use`](program.md#use) | `lib/vllm.sq` and the programs that `use` it |
| what to measure or check | [`observe`](statements.md#observe), [`gauge`](program.md#gauge), [`claim`](program.md#claim) | [the Kong use case](../use-cases/kong.md) |
