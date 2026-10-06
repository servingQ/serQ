# pyserq.Pool

One pool's report row. Obtain it from [`Report.pool`](report.md#pool),
`Report.pools_named` or `Report.pools`. An array has a row per member.
`Pool()` is not a public constructor; attributes are read-only.

## Attributes

| Attribute | Type | Description |
|---|---|---|
| `name` | `str` | Pool declaration name. |
| `index` | `int` or `None` | Array member index; `None` for a scalar pool. |
| `mean_used` | `float` | Time-average allocated units after warm-up. |
| `mean_cached` | `float` | Time-average cached units after warm-up. |
| `mean_queue` | `float` | Time-average number of waiting sessions after warm-up. |
| `mean_holders` | `float` | Time-average number of holders after warm-up. |
| `mean_wait` | `float` | Mean queue wait of admissions counted after warm-up. |
| `admissions` | `int` | Admissions over the whole run, including warm-up. |
| `evicted_entries` | `int` | Cache entries fully removed by eviction. |
| `evicted_units` | `float` | Units evicted, including partial eviction. |
| `preemptions` | `int` | Preemptions counted by this pool. |
| `spills` | `int` | Spills of evicted prefixes to another pool. |
| `rejected` | `int` | Requests rejected because static demand could never fit. |
| `stuck` | `int` | Repeated preemptions without progress past the previous preemption. |
| `over_cap` | `tuple[str, float]` or `None` | At run end, the waiting queue name and demand exceeding this pool's capacity; `None` when absent. |

Pool counters include warm-up; time averages and admission wait samples
use the measured interval.

See [pool statistics](../reference/cli.md#pool-statistics) for the report's
counter and diagnostic conventions.

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
pool = report.pool("slots")
assert pool is not None
assert pool.index is None
assert pool.admissions == 4
assert pool.over_cap is None
```

## See also

[`Report`](report.md), [`Stage`](stage.md), [pool declarations](../api/pool.md).
