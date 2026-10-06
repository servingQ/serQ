# pyserq.Gauge

Time-weighted statistics for one `gauge` declaration. Obtain it from
[`Report.gauge(name)`](report.md#gauge) or `Report.gauges`.
`Gauge()` is not a public constructor; attributes are read-only.

## Attributes

| Attribute | Type | Description |
|---|---|---|
| `name` | `str` | Gauge name. |
| `mean` | `float` | Time average over `[warmup, end]`. |
| `ci` | `float` | Batch-means 95% confidence-interval half-width, using 20 equal time windows. |
| `min` | `float` | Least value held for a positive duration after warm-up. |
| `max` | `float` | Greatest value held for a positive duration after warm-up. |
| `times` | `list[float]` | Change-point times, including before warm-up. |
| `values` | `list[float]` | Value held from each corresponding time until the next change. |

`times` and `values` have matching positions. Each access creates a new
list. The mean weights each value by how long it was held; averaging the
`values` list does not compute it.

## Examples

```python
import pyserq

source = """
fn main() {
  pool reqs { cap 2; }
  stage svc : fifo;
  workload {
    arrive batch(4);
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
gauge = report.gauge("occupied")
assert gauge is not None
points = list(zip(gauge.times, gauge.values))
assert gauge.max == 2.0
assert points
```

## See also

[`Report`](report.md), [`Observe`](observe.md), [`gauge` declaration](../api/program.md#gauge).
