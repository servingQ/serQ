# pyserq.run

```python
pyserq.run(program: Program) -> Report
```

Simulate a compiled program using its seed, workload and run settings.
Each call starts a fresh run and leaves the `Program` unchanged.

## Parameters

| Parameter | Type | Description |
|---|---|---|
| `program` | [`Program`](program.md) | Program returned by `compile` or `Program.from_json`. |

## Returns

[`Report`](report.md) — run metadata, statistics, sample records and JSON
output. The function releases the GIL while simulating, so calls from
Python threads can run in parallel.

## Raises

- `ValueError`: a trace cannot be loaded or the simulation fails, including
  an invalid runtime expression or a finite run that misses its deadline.
- `TypeError`: `program` is not a `Program`.

A failed claim is recorded in [`Report.json()`](report.md#json); inspect
its `claims` field to check the result.

## Examples

```python
import pyserq

source = """
fn main() {
  pool reqs { cap 2; }
  stage svc : fifo;
  workload {
    arrive batch(4);
    session { request; end; }
  }
  server {
    set t0 = now;
    hold reqs (1) { run svc (2); }
    observe response = now - t0;
  }
  gauge occupied = used(reqs);
}
"""
report = pyserq.run(pyserq.compile(source=source, horizon=10))
assert report.ended == 4
assert report.observe("response").samples == [2.0, 4.0, 6.0, 8.0]
```

## See also

[`compile`](compile.md), [`Report`](report.md), [finite runs](../api/program.md#a-finite-run).
