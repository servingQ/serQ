# pyserq.Report

The result of [`run`](run.md). `Report()` is not a public constructor.
Its attributes are read-only.

## Attributes

| Attribute | Type | Description |
|---|---|---|
| `serq_version` | `str` | Version of the interpreter that ran the program. |
| `horizon` | `float` | Configured simulation deadline. |
| `end` | `float` | Actual end time; may be earlier for a finite arrival-limited run. |
| `warmup` | `float` | Warm-up cutoff for measured statistics. |
| `seed` | `int` | Run seed. |
| `events` | `int` | Number of events processed. |
| `arrivals` | `int` | Sessions that arrived. |
| `ended` | `int` | Sessions that ended. |
| `turns` | `int` | Turns counted by the run. |
| `mean_live` | `float` | Time-average number of live sessions after warm-up. |
| `observes` | `dict[str, Observe]` | Observations in program order. A new dictionary on each access. |
| `gauges` | `dict[str, Gauge]` | Gauges in program order. A new dictionary on each access. |
| `stages` | `list[Stage]` | One row per stage or array member. A new list on each access. |
| `pools` | `list[Pool]` | One row per pool or array member. A new list on each access. |

Python float attributes preserve NaN and infinities. JSON encodes these as
`null`. The JSON also includes claim results and report metadata that do
not have Python attribute wrappers.

## Methods

| Method | Returns | Description |
|---|---|---|
| [`json()`](#json) | `str` | JSON summary matching the CLI. |
| [`observe(name)`](#observe) | `Observe` or `None` | Observation by name. |
| [`gauge(name)`](#gauge) | `Gauge` or `None` | Gauge by name. |
| [`stage(name)`](#stage) | `Stage` or `None` | First stage row with this name. |
| [`stages_named(name)`](#stages_named) | `list[Stage]` | All stage array members, in index order. |
| [`pool(name)`](#pool) | `Pool` or `None` | First pool row with this name. |
| [`pools_named(name)`](#pools_named) | `list[Pool]` | All pool array members, in index order. |

All lookup methods take `name: str`, the declaration's name. Use `"svc"`,
not `"svc[0]"`, for an array; select members by their `index`.
A non-string name raises `TypeError`. A missing name returns `None` for
single-object lookups or an empty list for plural lookups.

## json

```python
Report.json(self) -> str
```

Return the JSON summary produced by `serq run --json`. Parse it with
`json.loads`. Samples and gauge change points are available through the
objects below rather than embedded in this summary. The schema is
versioned by [`REPORT_VERSION`](../python.md#version-constants).

## observe

```python
Report.observe(self, name: str) -> Observe | None
```

Return the named [Observe](observe.md), or `None`.

## gauge

```python
Report.gauge(self, name: str) -> Gauge | None
```

Return the named [Gauge](gauge.md), or `None`.

## stage

```python
Report.stage(self, name: str) -> Stage | None
```

Return the first [Stage](stage.md) row with this name, or `None`. For an
array, use `stages_named` to inspect every member.

## stages_named

```python
Report.stages_named(self, name: str) -> list[Stage]
```

Return all [Stage](stage.md) rows with this name in index order, or `[]`.
A scalar stage has one row whose `index` is `None`.

## pool

```python
Report.pool(self, name: str) -> Pool | None
```

Return the first [Pool](pool.md) row with this name, or `None`.

## pools_named

```python
Report.pools_named(self, name: str) -> list[Pool]
```

Return all [Pool](pool.md) rows with this name in index order, or `[]`.
A scalar pool has one row whose `index` is `None`.

## Examples

```python
import pyserq

source = """
fn main() {
  pool slots { cap 2; }
  stage svc : fifo;
  workload { arrive batch(4); }
  session {
    set t0 = now;
    hold slots (1) { run svc (2); }
    observe latency = now - t0;
    end;
  }
  gauge occupied = used(slots);
  run { horizon 10; }
}
"""
report = pyserq.run(pyserq.compile(source=source))
import json

assert report.pool("slots").admissions == 4
assert report.stage("svc").completed == 4
assert report.observe("missing") is None
assert report.pools_named("missing") == []
summary = json.loads(report.json())
assert summary["ended"] == report.ended
```

## See also

[`run`](run.md), [`Observe`](observe.md), [`Gauge`](gauge.md),
[`Stage`](stage.md), [`Pool`](pool.md), [report fields](../reference/cli.md#the-json-report).
