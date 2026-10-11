# Python Reference

`pyserq` compiles, simulates and draws serQ programs in Python. It uses the
same interpreter as the CLI and returns objects for inspecting a run.

## Install

```bash
pip install pyserq
```

Requires Python 3.9 or newer. To include development releases, use
`pip install --pre pyserq`.

## Run a program

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
response = report.observe("response")
assert response is not None
print(response.mean)
```

## API

### Compilation and execution

| Function or class | Description |
|---|---|
| [`pyserq.compile`](python/compile.md) | Compile a file or source text with parameter and run overrides. |
| [`pyserq.Program`](python/program.md) | A compiled program; serialize or load its IR. |
| [`pyserq.run`](python/run.md) | Simulate a compiled program and return a report. |

### Results

| Class | Description |
|---|---|
| [`pyserq.Report`](python/report.md) | Run metadata, named lookups and JSON output. |
| [`pyserq.Observe`](python/observe.md) | Sample statistics and per-session records. |
| [`pyserq.Gauge`](python/gauge.md) | Time-weighted statistics and change points. |
| [`pyserq.Stage`](python/stage.md) | Queue, service, iteration and inter-token statistics. |
| [`pyserq.Pool`](python/pool.md) | Allocation, cache, admission and preemption statistics. |

### Visualization and inputs

| Function or class | Description |
|---|---|
| [`pyserq.draw`](python/draw.md) | Render a deployment as TikZ or SVG. |
| [`pyserq.read_trace`](python/read-trace.md) | Read a replay CSV into sessions and turns. |
| [`pyserq.Rng`](python/rng.md) | Draw from the interpreter's random-number generator. |

## Version constants

| Name | Type | Meaning |
|---|---|---|
| `pyserq.__version__` | `str` | Version of the installed serQ implementation. |
| `pyserq.IR_VERSION` | `int` | IR version accepted by this build. See [IR stability](ir.md#stability). |
| `pyserq.REPORT_VERSION` | `int` | Schema version of [`Report.json()`](python/report.md#json). |

## Errors and types

Compilation, validation, trace-loading and simulation errors raise
`ValueError` with serQ's diagnostic. Wrong Python argument types raise
`TypeError`; integers outside the accepted unsigned range raise
`OverflowError`. Individual entries describe additional conditions.

Signatures below use Python type notation to describe accepted values and
returns. A `*` marks keyword-only arguments. `None` for a compile override
keeps the program's setting; it does not select a new default.

For language syntax, see the [language API](api/index.md). For command-line
options and report definitions, see the [CLI reference](reference/cli.md).
