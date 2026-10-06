---
hide:
  - navigation
---

# ![serQ](assets/brand/serq-logo.svg#only-light){ width="300" } ![serQ](assets/brand/serq-logo-dark.svg#only-dark){ width="300" }

*pronounced "ser-Q": **se**rving + **Q**ueue*

**serQ describes an LLM serving deployment as a program:** its memory
pools, engines, workload and scheduling policy. Use the same program to
simulate performance, state claims for Lean proofs and draw the deployment.
Programs supported by the [vLLM target](reference/cli.md#the-vllm-target)
also compile to a scheduler configuration.

Start with [installation and a first run](getting-started.md), or build a
serving model step by step in the [tutorial](tutorial/index.md).

## What you can do with it

<div class="grid cards" markdown>

- **Simulation**

    `serq run` executes the program as a discrete-event simulation and
    reports each stage's throughput, utilisation and waits, an engine's
    inter-token latency, and whatever the program observes (time to first
    token, say) with a confidence interval. `--set` and `--def` change a
    constant or a law without editing the file, for sweeps.
    [Getting started](getting-started.md), [use cases](use-cases/index.md#simulation)

- **Formal verification**

    Write claims in a program and prove them about its Lean model, under
    explicit assumptions. For supported programs and claims, Lean statements
    are generated from the IR; their proofs are written separately and
    checked by Lean. The simulator also checks claims on the path it runs.
    The interpreter and Lean semantics are tested for agreement; their
    equivalence is not proved.
    [The Lean model](lean.md), [formal verification examples](use-cases/index.md#formal-verification)

- **Deployment visualization**

    `serq draw` renders the program as a queueing network, in TikZ or SVG.
    Regenerate the figure when the program changes.
    [Visualization](visualization/index.md)

</div>
