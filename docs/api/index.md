# API Reference

Look up syntax, argument types and evaluation rules here. For the complete
operational semantics, see [The language](../language.md).

## API index {#by-construct}

| Page | Contents |
|---|---|
| [Program](program.md) | `let`, `def`, `use`, `pool`, `stage`, `workload`, `session`, `server`, `gauge`, `claim` |
| [std/args](std/args.md) | `args.number`, the program's inputs |
| [std/kvt](std/kvt.md) | `kvt.nixl_pull`, `kvt.nixl_push` |
| [std/share](std/share.md) | `share.maxmin`, `share.bottleneck`, how the flows of a fabric divide it |
| [Pool](pool.md) | the options of `pool` |
| [Stage](stage.md) | `fifo`, `ps`, `delay` |
| [Engine](engine.md) | `device`, `engine … on`, `tokens cap`, `connector`, `scheduler`, `worker`, `pool … on`, a request and its admission |
| [Workload](workload.md) | `arrive`, `trace`, `init`, `turn`, `hidden` |
| [Statements](statements.md) | the kernel: `hold`, `run`, `grow`, `if`, … |
| [Serving vocabulary](serving.md) | `transfer`, `tool` |
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
| `Size`, `Value` | Ordinary quantities: workload-owned request/client attributes and server bookkeeping, respectively. See [attribute types](attributes.md#sizes-values-and-costs). |
| `Cost(resource)` | An explicitly interpreted resource quantity. Primitive resource operations require their own cost type. See [`cost`](functions.md#cost). |
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
| arrivals, turns and think time | [`arrive`](workload.md#arrive), [`turn`](workload.md#turn), [`session`](program.md#session); think time as [`tool`](serving.md#tool), on the workload's own [`delay`](stage.md#delay) | [the workloads use case](../use-cases/workloads.md) |
| replaying a production trace | [`trace`](workload.md#trace) | `examples/replay/vllm_replay.sq` ([measured replay](../use-cases/vllm.md#measured-replay)) |
| memory a request holds while it runs | [`pool`](pool.md), [`hold`](statements.md#hold) | [tutorial 2](../tutorial/02-memory.md) |
| what the scheduler reads at admission | [`hold`](statements.md#hold)'s header, [moments](context.md) | `examples/multi-turn/vllm.sq` (`request.known`, `request.hit`) |
| a prefix cache across turns | [`cache`](statements.md#hold), [`evict`](pool.md#evict), `cached` ([attributes](attributes.md)), [`drop`](statements.md#drop); [`cachedin`](functions.md#pool) for the lookup at admission | [tutorial 4](../tutorial/04-prefix-cache.md); `examples/multi-turn/vllm.sq` for `reuse` |
| an offload tier for evicted prefixes | [`spill`](pool.md#spill) | `tests/pool_semantics.rs` |
| continuous batching and chunked prefill | [`engine`](engine.md) (`tokens cap`, `each at most`, `worker`), an engine's [`request`](../language.md#an-engines-request) (`prefill (e)`, `decode (e)`, `growing`, `taking`) | [the vLLM use case](../use-cases/vllm.md); [tutorial 5](../tutorial/05-the-engine.md) |
| the order requests are admitted and served in | [`queue by`](pool.md#queue), [`advance running`](engine.md#orders-and-only) (`decode first`, `by (…)`) | `examples/multi-turn/replica.sq` (`decode first`); `examples/papers/kong_svf.sq` (`queue by`) |
| preemption and recompute | [`preempt lifo`](pool.md#preempt), `computed` ([attributes](attributes.md)) | `examples/multi-turn/vllm.sq` (`request.known`) |
| the scheduler's waiting loop and its budget | [`pool … on`](engine.md#pools-on-a-device-or-an-engine), [`admit waiting`](engine.md#schedule), [`budget_left`](functions.md#engine) | `examples/multi-turn/vllm.sq` (`pool reqs on vllm`, `budget_left`) |
| what the scheduler may not read | [`hidden`](workload.md#hidden) | every vLLM program hides the output length (`o`; `out` in the replay) |
| prefill/decode disaggregation | [`lease`](statements.md#hold), [`release`](statements.md#release), [`load`](statements.md#load), [`transfer … from … to`](serving.md#transfer-from-to), [`kvt.nixl_pull`, `kvt.nixl_push`](std/kvt.md#kvtnixl_pull-and-kvtnixl_push), [`share.maxmin`](std/share.md) | [the P/D use case](../use-cases/pd.md) |
| an L2 KV cache outside the engine (LMCache, Mooncake) | the engine's [`connector`](engine.md#connector), the `l2` role ([Queues](../language.md#queues)) | `examples/multi-turn/vllm_lmcache.sq` |
| a routing policy | [`choose`](statements.md#choose), [observables](functions.md#observables) | `examples/multi-turn/routing.sq` |
| what to measure or check | [`observe`](statements.md#observe), [`gauge`](program.md#gauge), [`claim`](program.md#claim) | [the Kong use case](../use-cases/kong.md) |
