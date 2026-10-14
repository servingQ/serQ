# pyserq.Claim

What a run found of one `claim` declaration, by the names of the `claims`
entries of `serq run --json`. Obtain it from
[`Report.claim(name)`](report.md#claim) or `Report.claims`.
`Claim()` is not a public constructor; attributes are read-only.

## Attributes

| Attribute | Type | Description |
|---|---|---|
| `name` | `str` | Claim name. |
| `kind` | `str` | `every_iteration`, `some_iteration` or `at_end`. |
| `result` | `str` | `holds`, `fails`, `witnessed`, `not_witnessed`, `not_evaluated` or `out_of_scope`. |
| `checked` | `int` | Iterations the claim was read at, or 1 for a claim read at the end. |
| `failures` | `int` | Of those, the ones that read 0. |
| `first` | `float` or `None` | The first failure, or a `some` claim's first witness. |
| `note` | `str` or `None` | Why the claim was not read: the sessions live at the end, or the session that failed `given`. |

A claim reads the whole run, warm-up included.

## Examples

```python
import pyserq

source = """
fn main() {
  pool reqs { cap 2; }
  stage svc : fifo;
  server {
    set t0 = now;
    hold reqs (cost(reqs, 1)) { run svc (cost(svc, 2)); }
    observe response = now - t0;
  }
  claim completed : at end (count(response) == 4);
}
"""
workload = """
workload {
  arrive batch(4);
}
"""
report = pyserq.run(pyserq.compile(source=source, workload_source=workload, horizon=10))
claim = report.claim("completed")
assert claim.kind == "at_end" and claim.result == "holds"
assert list(report.claims) == ["completed"]
```

## See also

[`Report`](report.md), [`claim` declaration](../api/program.md#claim).
