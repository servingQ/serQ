# pyserq.read_trace

```python
pyserq.read_trace(path) -> list[list[tuple[float, float, float, float]]]
```

Read a replay CSV using the same loader as a simulation.

## Parameters

| Parameter | Type | Description |
|---|---|---|
| `path` | `str` or `os.PathLike` | CSV file. A relative path resolves from the current working directory. |

## Returns

A list of sessions, each a list of turns. Each turn is the tuple
`(new, out, think, forced)`; all four values are floats. Contiguous rows with the same session identifier form one session; turns
remain in file order. The loader does not sort by the turn column. Neither
identifier is included in the returned tuples.

The CSV has columns `session,turn,new,out,think` and an optional `forced`
column, which defaults to 0. See [`trace`](../api/workload.md#trace) for
how a workload samples or cycles through this corpus.

## Raises

`ValueError` if the file cannot be read or is not a valid trace;
`TypeError` if `path` is not a supported path value.

## Examples

```python
from pathlib import Path
from tempfile import TemporaryDirectory
import pyserq

with TemporaryDirectory() as directory:
    path = Path(directory) / "trace.csv"
    path.write_text("session,turn,new,out,think\n0,0,16,4,0\n", encoding="utf-8")
    sessions = pyserq.read_trace(path)
    assert sessions == [[(16.0, 4.0, 0.0, 0.0)]]
```

## See also

[`compile(trace=...)`](compile.md), [trace workloads](../api/workload.md#trace).
