# pyserq.Program

A compiled serQ program. Create it with [`compile`](compile.md) or
[`Program.from_json`](#from_json); `Program()` is not a public constructor.
The object is immutable and can be run more than once.

| Method | Description |
|---|---|
| [`to_json()`](#to_json) | Serialize the program's IR. |
| [`from_json(s)`](#from_json) | Load and validate IR JSON. |

## to_json

```python
Program.to_json(self) -> str
```

**Returns:** the program's IR as a JSON string, in the format of `serq ir`.
There are no parameters beyond `self`.

The string contains the program, not its source-directory context or trace
file contents. See [IR stability](../ir.md#stability) for version rules.

## from_json

```python
Program.from_json(s: str) -> Program
```

Static method. Parse and validate a serialized IR program.

**Parameters:** `s` (`str`) — IR JSON, such as the output of `to_json()`.

**Returns:** a new `Program`. Relative traces resolve from the current
working directory when it runs.

**Raises:** `ValueError` for malformed JSON, an unsupported IR version or
an invalid program; `TypeError` for a non-string argument.

## Examples

```python
import pyserq

program = pyserq.compile(source="""
fn main() {
  stage svc : delay;
  workload {
    arrive batch(1);
  }
  server {
    run svc (2);
    observe elapsed = now;
  }
}
""", horizon=10)
restored = pyserq.Program.from_json(program.to_json())
assert pyserq.run(restored).json() == pyserq.run(program).json()
```

## See also

[`compile`](compile.md), [`run`](run.md), [the IR](../ir.md).
