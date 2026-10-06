# pyserq.Observe

Statistics and sample records for one `observe` declaration. Obtain it from
[`Report.observe(name)`](report.md#observe) or `Report.observes`.
`Observe()` is not a public constructor; attributes are read-only.

## Attributes

| Attribute | Type | Description |
|---|---|---|
| `name` | `str` | Observation name. |
| `count` | `int` | Number of recorded samples after warm-up. |
| `mean` | `float` | Sample mean. |
| `ci` | `float` | Batch-means 95% confidence-interval half-width; positive infinity below 40 samples. |
| `cv2` | `float` | Squared coefficient of variation. |
| `p99` | `float` | 99th percentile of the recorded values. |
| `samples` | `list[float]` | Values in recording order. |
| `times` | `list[float]` | Simulation time of each sample. |
| `sessions` | `list[int]` | Session serial of each sample. |
| `turns` | `list[int]` | Turn number of each sample. |

The four lists have matching positions. Each property access creates a new
list; bind it once when processing many records. A CI is a half-width,
not a pair of endpoints. When finite, its interval is `mean ± ci`.
Statistics without a finite value remain NaN or infinity in Python.

## Examples

```python
import pyserq

source = """
fn main() {
  pool slots { cap 2; }
  stage svc : fifo;
  workload {
    arrive batch(4);
    session { request; end; }
  }
  server {
    set t0 = now;
    hold slots (1) { run svc (2); }
    observe latency = now - t0;
  }
  gauge occupied = used(slots);
}
"""
report = pyserq.run(pyserq.compile(source=source, horizon=10))
observation = report.observe("latency")
assert observation is not None
records = list(zip(
    observation.times, observation.sessions,
    observation.turns, observation.samples,
))
assert len(records) == observation.count == 4
```

## See also

[`Report`](report.md), [`Gauge`](gauge.md), [`observe` statement](../api/statements.md#observe).
