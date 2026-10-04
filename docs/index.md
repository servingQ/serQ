# Welcome to serQ

*pronounced "ser-Q": **se**rving + **Q**ueue*

**serQ is a language in which an LLM serving deployment is a program.** Its
memory pools, its engines, the traffic that arrives and the path every
request takes through them are written down once. That one program is then
simulated, checked against the real scheduler, proved about in Lean, and
drawn.

## Why serQ exists

A serving system is usually described three times, and the three are never
reconciled: a paper's queueing model, the scheduler's source code, and
whatever load test last ran against it. Each answers a different question,
and none of them can be checked against the others.

serQ replaces the three with one program. Its definition is an intermediate
representation, the [IR](ir.md): the simulator runs it, the Lean model is
generated from it, and the oracle tests read it. The `.sq` text you write is
one frontend that compiles to it. Because there is only one description,
the simulation, the proofs and the figure cannot disagree with one another.

## What you can do with it

<div class="grid cards" markdown>

- **Write a deployment**

    Pools of KV blocks or request slots, step engines with a token budget
    and a cost model, a workload of sessions and turns, and the policy a
    request runs. The [tutorial](tutorial/index.md) builds one up in six
    runnable programs.

- **Simulate it**

    `serq run` executes the program as a discrete-event simulation and
    reports each stage's throughput, utilisation and waits, an engine's
    inter-token latency, and whatever the program observes (time to first
    token, say) with a confidence interval. `--set` and `--def` change a
    constant or a law without editing the file, for sweeps.
    [Getting started](getting-started.md)

- **Check it against the real scheduler**

    The vLLM v1 program agrees with the scheduler at the pinned revision on
    six deterministic scenarios, and on all 3 321 requests of a 333-session
    trace: send time, first-token time and cached tokens.
    [How serQ is checked](validation.md)

- **Prove things about it**

    The same IR is a program in a Lean semantics. The memory invariant
    `allocated + cached ≤ cap` is a theorem for every pool, and the vLLM
    scenarios are theorems checked by the kernel.
    [The Lean model](lean.md)

- **Draw it**

    `serq draw` renders the program as a queueing network, in TikZ or SVG.
    The figure is generated from the program, so it cannot fall out of date.
    [Visualization](visualization/index.md)

- **Use it from Python**

    `pip install pyserq` gives you compile, run and draw in process, with
    the report as Python objects.
    [pyserq](python.md)

</div>

## A program

This is the request that `examples/multi-turn/vllm.sq` serves: vLLM v1's
admission, prefix-cache hit, chunked prefill and decode, as one scoped
`hold`.

```serq title="lib/vllm.sq"
--8<-- "lib/vllm.sq"
```

And this is what `serq draw` makes of the whole program: one step engine,
with its request slots and KV blocks.

![vLLM v1 as a queueing network](assets/vllm.deployment.svg)

The program is a restricted, synchronous model of the scheduler. The
[vLLM use case](use-cases/vllm.md) says what it covers and what it leaves out.

## The organising idea

A serving deployment does four things to a request:

1. makes it **wait for a resource**,
2. **runs** it on a stage,
3. **frees** the resource, possibly keeping a prefix cached,
4. **sends it somewhere next**.

serQ makes the first three one scoped statement, `hold p (u) { … } cache (ℓ)`,
and generalises "resource" so that KV memory, request slots, live-session
caps and offload tiers are all the same kind of object: a **pool**. The fourth
is ordinary control flow: `branch`, `loop`, `end`.

## Where to start

| If you want to | Go to |
|---|---|
| build it and run something | [Getting started](getting-started.md) |
| learn the language from scratch | [Tutorial](tutorial/index.md): six chapters, each a runnable program |
| see a real system written in it | [vLLM](use-cases/vllm.md), [vendor plugins](use-cases/index.md#vendor-plugins), [prefill/decode over NIXL](use-cases/pd.md) |
| see one engine serve single-turn, chat and agent traffic | [Different workloads](use-cases/workloads.md) |
| sweep a parameter, compare two designs, or take a program to Lean | [Working with a program](development.md) |
| look something up | [Language reference](api/index.md), [Cheatsheet](reference/cheatsheet.md), [CLI](reference/cli.md), [Python](python.md) |
| know why any of this should be believed | [How serQ is checked](validation.md) |
| know why the language is the way it is | [Design](design/index.md) |

!!! note "The specification"
    [The language](language.md) and [The IR](ir.md) are the
    specification-grade documents: complete and dense. The tutorial is the
    way in, those are what you read afterwards, and the
    [language reference](api/index.md) is where you look a construct up.
