# pyserq.draw

```python
pyserq.draw(path=None, *, source=None, sets={}, defs={}, format="tikz") -> str
```

Render the [deployment view](../visualization/index.md). Supply exactly one
of `path` and `source`. This function accepts source or an IR file; it does
not accept a compiled `Program`.

## Parameters

| Parameter | Type | Default | Description |
|---|---|---|---|
| `path` | `str` or `os.PathLike` | `None` | A `.sq` source file or `.json` IR file. |

## Keyword arguments

| Parameter | Type | Default | Description |
|---|---|---|---|
| `source` | `str` | `None` | Source text, instead of `path`: give exactly one. It may import the built-in [`std/args`](../api/std/args.md) and [`std/kvt`](../api/std/kvt.md). |
| `sets` | `dict[str, float or str]` | `{}` | Declared program inputs, as in [`compile`](compile.md). |
| `defs` | `dict[str, str]` | `{}` | Expression-definition overrides. |
| `format` | `str` | `"tikz"` | `"tikz"` or `"svg"`. |

## Returns

`str` — a TikZ `tikzpicture` or a standalone SVG document. It does not write
a file. In a notebook, pass SVG output to `IPython.display.SVG`.

For a program split into a workload and a server, the diagram shows one
request's route, from source or from its JSON IR alike: the IR marks each
statement as the workload's or the server's. JSON IR rejects `sets` and
`defs`.

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
  workload {
    arrive batch(1);
  }
  server {
    run svc (cost(svc, 2));
  }
}
""", format="svg")
Path("deployment.svg").write_text(svg, encoding="utf-8")
```

## See also

[Deployment visualization](../visualization/deployment.md), [`compile`](compile.md).
