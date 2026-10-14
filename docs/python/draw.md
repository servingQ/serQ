# pyserq.draw

```python
pyserq.draw(path=None, *, source=None, workload=None, workload_source=None,
            sets={}, defs={}, format="tikz") -> str
```

Render the [deployment view](../visualization/index.md). Supply exactly one
of `path` and `source`, and for program text one of `workload` and
`workload_source`, as for [`compile`](compile.md): the figure starts at the
arrival.

## Parameters

| Parameter | Type | Default | Description |
|---|---|---|---|
| `path` | `str`, `os.PathLike` or [`Program`](program.md) | `None` | A `.sq` source file, a `.json` IR file, or a compiled program (the name stays `path` for callers that pass it by keyword). |

## Keyword arguments

| Parameter | Type | Default | Description |
|---|---|---|---|
| `source` | `str` | `None` | Source text, instead of `path`: give exactly one. It may import the built-in [`std/args`](../api/std/args.md) and [`std/kvt`](../api/std/kvt.md). |
| `workload` | `str` or `os.PathLike` | `None` | The workload file the deployment runs, as in [`compile`](compile.md); refused with a `.json` IR or a `Program`. |
| `workload_source` | `str` | `None` | Workload text, instead of `workload`. |
| `sets` | `dict[str, float or str]` | `{}` | Declared program inputs, as in [`compile`](compile.md). |
| `defs` | `dict[str, str]` | `{}` | Expression-definition overrides. |
| `format` | `str` | `"tikz"` | `"tikz"` or `"svg"`. |

## Returns

`str` — a TikZ `tikzpicture` or a standalone SVG document. It does not write
a file. In a notebook, pass SVG output to `IPython.display.SVG`.

The diagram shows one request's route, from the deployment and its
workload, from their JSON IR or from a compiled `Program` alike: the IR
marks each statement as the workload's or the server's. JSON IR and a
`Program` hold their workload already and reject `workload`,
`workload_source`, `sets` and `defs`: their constants are already folded
and their definitions expanded.

## Raises

`ValueError` for missing or conflicting inputs, an unreadable file, invalid
source/IR or overrides, or an unknown format. Wrong argument types raise
`TypeError`.

## Examples

```python
from pathlib import Path
import pyserq

svg = pyserq.draw(source="""
fn main() {
  stage svc : fifo;
  server {
    run svc (cost(svc, 2));
  }
}
""", workload_source="""
workload {
  arrive batch(1);
}
""", format="svg")
Path("deployment.svg").write_text(svg, encoding="utf-8")
```

A program already compiled draws the same figure as its file drawn with
the same `sets` and `defs`; the run settings (`horizon`, `warmup`, `seed`,
`arrivals`, `trace`) do not enter the figure:

```python
mg1 = dict(workload="examples/workloads/mg1.sq", sets={"lam": 0.3})
prog = pyserq.compile("examples/single-turn/mg1.sq", horizon=100, **mg1)
assert pyserq.draw(prog) == pyserq.draw("examples/single-turn/mg1.sq", **mg1)
```

## See also

[Deployment visualization](../visualization/deployment.md), [`compile`](compile.md).
