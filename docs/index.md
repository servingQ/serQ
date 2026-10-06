---
hide:
  - navigation
---

# ![serQ](assets/brand/serq-logo.svg#only-light){ width="300" } ![serQ](assets/brand/serq-logo-dark.svg#only-dark){ width="300" }

*pronounced "ser-Q": **se**rving + **Q**ueue*

**serQ describes an LLM serving deployment as a program:** its memory
pools, engines, workload and scheduling policy. Use the same program to
simulate performance, state claims for Lean proofs and draw the deployment.

Start with [installation and a first run](getting-started.md), or build a
serving model step by step in the [tutorial](tutorial/index.md).

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

    Write a `claim` in the program and check it on a simulated run.
    For supported programs, Lean proofs establish claims over all paths
    covered by their assumptions. The interpreter and Lean semantics are
    tested for agreement; their equivalence is not proved.
    [The Lean model](lean.md), [formal verification examples](use-cases/index.md#formal-verification)

- **Draw it**

    `serq draw` renders the program as a queueing network, in TikZ or SVG.
    Regenerate the figure when the program changes.
    [Visualization](visualization/index.md)

- **Use it from Python**

    `pip install pyserq` gives you compile, run and draw in process, with
    the report as Python objects.
    [pyserq](python.md)

</div>
