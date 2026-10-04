---
hide:
  - navigation
---

# Welcome to serQ

*pronounced "ser-Q": **se**rving + **Q**ueue*

**serQ is a language in which an LLM serving deployment is a program.** Its
memory pools, its engines, the traffic that arrives and the path every
request takes through them are written down once. That one program is then
simulated, proved about in Lean, and drawn; the vLLM program is checked
against the real scheduler request for request.

## Why serQ exists

A serving system is usually described three times, and the three are never
reconciled: a paper's queueing model, the scheduler's source code, and
whatever load test last ran against it. Each answers a different question,
and none of them can be checked against the others.

serQ replaces the three with one program. Its definition is an intermediate
representation, the [IR](ir.md): the simulator runs it, the Lean model is
generated from it, and the oracle tests read it. The `.sq` text you write is
one frontend that compiles to it. Because they read the same IR, a change to
the program reaches the simulation, the Lean statements and the figure at
once. That the interpreter and the Lean semantics agree is tested (the
oracle scenarios, random differential runs), not proved.

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
    [Getting started](getting-started.md), [use cases](use-cases/index.md#simulation)

- **State claims and prove them**

    A `claim` is a proposition about every path of the program, written in
    the program: the simulator checks it on the path it runs, and for a
    program inside Lean's fragment a Lean proof covers every path. Three
    papers' propositions are proved this way, and the memory invariant `allocated + cached ≤ cap` is a theorem
    of the pool model.
    [The Lean model](lean.md), [Claims](design/claims.md), [use cases](use-cases/index.md#formal-verification)

- **Draw it**

    `serq draw` renders the program as a queueing network, in TikZ or SVG.
    The figure is generated from the program, so it cannot fall out of date.
    [Visualization](visualization/index.md)

- **Use it from Python**

    `pip install pyserq` gives you compile, run and draw in process, with
    the report as Python objects.
    [pyserq](python.md)

</div>
