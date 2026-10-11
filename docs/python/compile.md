# pyserq.compile

```python
pyserq.compile(
    path=None, *, source=None, sets={}, defs={}, seed=None,
    horizon=None, warmup=None, arrivals=None, trace=None,
) -> Program
```

Compile a program file or source text into a validated [Program](program.md).
Supply exactly one of `path` and `source`. For source models, supply `horizon=`;
execution settings belong to this call, not to the model text. JSON IR already
contains its execution settings.

## Parameters

| Parameter | Type | Default | Description |
|---|---|---|---|
| `path` | `str` or `os.PathLike` | `None` | Path to a `.sq` source file or a `.json` IR file. |

## Keyword arguments

| Parameter | Type | Default | Description |
|---|---|---|---|
| `source` | `str` | `None` | Source text, instead of a file. |
| `sets` | `dict[str, float or str]` | `{}` | Supply inputs declared with `args.number` by their external name; plain `let` constants cannot be overridden. Numbers are literal values; strings are serQ expressions. Integers are also accepted as numbers. |
| `defs` | `dict[str, str]` | `{}` | Replace expression `def` bodies by name, as with CLI `--def`. |
| `seed` | `int` | `None` | Set the seed (default 1); from 0 through `2**64 - 1`. |
| `horizon` | `float` | `None` | Required for source models: the simulation deadline in the program's clock units. |
| `warmup` | `float` | `None` | Set the warm-up duration (default 0); must be below the horizon. |
| `arrivals` | `int` | `None` | Positive arrival limit for an open workload. The run must receive this many arrivals and finish their sessions before the deadline. |
| `trace` | `str` or `os.PathLike` | `None` | Override the replay CSV path, relative to the current working directory. |

## Returns

[`Program`](program.md) — a compiled program ready for [`run`](run.md) or
IR serialization. Compilation does not simulate it.

## Raises

- `ValueError`: both or neither input is supplied; the file cannot be read;
  parsing, linking or validation fails; an override is invalid.
- `TypeError` or `OverflowError`: an argument cannot be converted to its
  required Python/Rust type.

## Notes

`sets` accepts infinities as `inf` or `-inf`; NaN is rejected. With JSON IR
input, `sets` and `defs` are rejected; run overrides remain available.

Text supplied with `source=` can import the built-in `std/args` library.
For relative file imports, pass `path` so the source directory is known.
A relative trace named in a source file is resolved beside that file.
With `source=`, a `trace=` override, or `Program.from_json`, traces resolve
from the current working directory. Trace contents are read when the
program runs.

## Examples

```python
import pyserq

source = """
use "std/args";

fn main() {
  let duration = args.number("duration", 2);
  stage svc : delay;
  workload {
    arrive batch(1);
  }
  server {
    run svc (duration);
    observe elapsed = now;
  }
}
"""
program = pyserq.compile(source=source, sets={"duration": 3}, seed=10, horizon=10)
report = pyserq.run(program)
assert report.observe("elapsed").mean == 3.0
```

From a repository checkout:

```python
program = pyserq.compile(
    "examples/single-turn/mg1.sq",
    sets={"lam": 0.8, "law": 1},
    seed=10,
    horizon=250000, warmup=25000,
)
```

## See also

[`Program`](program.md), [`run`](run.md), [run settings](../api/program.md#run).
