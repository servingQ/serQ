# pyserq.Stage

One stage's report row. Obtain it from [`Report.stage`](report.md#stage),
`Report.stages_named` or `Report.stages`. An array has a row per member.
`Stage()` is not a public constructor; attributes are read-only.

## Attributes

| Attribute | Type | Description |
|---|---|---|
| `name` | `str` | Stage declaration name. |
| `index` | `int` or `None` | Array member index; `None` for a scalar stage. |
| `mean_number` | `float` | Time-average jobs present, waiting or in service. |
| `utilization` | `float` | Fraction of measured time with at least one job; for shared stages, average fraction of capacity carried by flows. |
| `completed` | `int` | Jobs completed after warm-up. |
| `throughput` | `float` | Completions per measured clock unit. |
| `mean_wait` | `float` | Mean wait before service. |
| `mean_service` | `float` | Mean time in service. |

### Step-engine statistics

| Attribute | Type | Description |
|---|---|---|
| `iterations` | `int` | Iterations over the whole run, including warm-up; 0 on other stage kinds. |
| `prefill_only` | `float` | Fraction of measured time running prefill-only iterations. |
| `decode_only` | `float` | Fraction running decode-only iterations. |
| `mixed` | `float` | Fraction running mixed prefill/decode iterations. |
| `mean_decodes` | `float` | Time-average number of decoders in the running iteration, 0 while idle. |
| `mean_decode_batch` | `float` | Mean decoder count over decode-carrying iterations started after warm-up. |
| `mean_decode_step` | `float` | Mean duration of those decode-carrying iterations. |
| `mean_itl` | `float` | Mean gap between successive tokens of a turn ending on this stage after warm-up. |
| `itl_p50` | `float` | Median inter-token gap, within 0.5%. |
| `itl_p99` | `float` | 99th percentile inter-token gap, within 0.5%. |
| `idle_with_work` | `bool` | The run ended with work waiting and the last iteration attempt scheduling nothing. |

The batch-time fractions and `mean_decodes` are 0 for other stage kinds.
Decode-batch and inter-token statistics are NaN when there are no matching
iterations or gaps. A token gap can include transfer, queueing or
preemption time. See [stage statistics](../reference/cli.md#stage-statistics).

## Examples

```python
import pyserq

report = pyserq.run(pyserq.compile(source="""
fn main() {
  stage svc[2] : fifo;
  workload {
    arrive batch(1);
    session { request; end; }
  }
  server {
    run svc[1] (2);
  }
}
""", horizon=10))
rows = report.stages_named("svc")
assert [row.index for row in rows] == [0, 1]
assert rows[1].completed == 1
```

## See also

[`Report`](report.md), [`Pool`](pool.md), [stage declarations](../api/stage.md).
