# Visualization

Generate a deployment diagram from a program and its workload, or from its IR:

```bash
serq draw examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq --format svg --out vllm.svg
```

![vLLM deployment](../assets/vllm.deployment.svg)

The [deployment view](deployment.md) shows stages, memory pools, instance
boundaries and request flow. Drawing does not run the simulation; the same
IR produces the same diagram.

!!! warning "Experimental notation"
    Diagram notation, options and output may change between releases.

## Formats

- **TikZ** (default): a `tikzpicture` for a LaTeX document using
  `\usepackage{tikz}`. Include the generated file with `\input`.
- **SVG**: a standalone image with light- and dark-mode colors.

Output goes to stdout unless `--out` names a file. Program inputs (`--name value`) and `--def`
apply the same overrides as for simulation. Python's
[`pyserq.draw`](../python.md) returns the diagram as text.
See the [CLI reference](../reference/cli.md) for all options.

## Limitations {#what-is-not-done}

- A pool held in separate places may appear in several frames or
  enclosures. Its name identifies the shared resource.
- Stations occupy one row; large deployments can produce wide diagrams.
- Long expressions are shortened with `~`.
- Labels show values: `cap blocks * bs` shows `cap 160000`. The
  [legend](deployment.md#the-legend) under the figure names each constant
  with its value, but a label does not say which constants it was computed
  from: `cap 16` and `block 16` are both 16.
- The deployment view does not show an iteration-by-iteration trace.
