# Python: pyserq

`pyserq` runs serQ in process. It shows what the [IR](ir.md) is the
definition of: a program is compiled, from a file or from text, to its IR,
and the IR is run. Nothing of the text frontend is exposed.

```python
import pyserq

p = pyserq.compile("examples/single-turn/mg1.sq", sets={"lam": 0.8, "law": 1}, seed=10)
r = pyserq.run(p)            # the GIL is released while it runs
r.json()                     # what `serq run --json` prints
values, times, sessions, turns = r.observe("sojourn")   # what `--dump` writes
pyserq.read_trace("examples/replay/data/short_base.csv")  # the sessions a replay draws from
```

| | |
|---|---|
| `compile(path=None, *, source=None, sets={}, seed, horizon, warmup, arrivals, trace)` | A program file or text to its IR, with the overrides of `serq run`. A number in `sets` is that number (an infinity is `inf`; NaN is refused); a string is an expression. A relative trace is read next to the program file for `compile(path)`, and from the current directory with `trace=`, `source=` or `Program.from_json`, as `serq run` does. |
| `Program.to_json()`, `Program.from_json(s)` | The IR as JSON (`serq ir`), and back. |
| `run(program)` | A run. Runs in threads proceed in parallel. |
| `Report.json()` | The summary `serq run --json` prints. |
| `Report.observe(name)` | `(values, times, sessions, turns)` of one observation. |
| `read_trace(path)` | A trace's sessions, each a list of its turns `(new, out, think, forced)`: the corpus a replay draws its sessions from, read as a replay reads it. A relative path is read from the current directory. |
| `IR_VERSION`, `REPORT_VERSION`, `__version__` | The IR it reads, the shape of `Report.json()` (its field names; a change bumps it), and the serq version it is. |

A program that does not compile or run raises `ValueError` with serQ's message; an argument of the wrong type (`seed=-1`, `sets={"x": None}`) raises `TypeError` or `OverflowError`, as Python does.

## Install

A release carries one wheel per platform (linux x86_64, macOS arm64), with
the version of its tag (`v0.1.0-rc6` is `pyserq 0.1.0rc6`). The repository is
private, so download the wheel with `gh release download` and install it:

```bash
gh release download v0.1.0-rc6 -R vrvrv/serQ -p 'pyserq-*.whl' -D wheels
pip install wheels/pyserq-*manylinux*.whl      # or the macosx one
```

Or build it from a checkout: `pip install maturin && maturin develop -m pyserq/Cargo.toml`.

`pyserq/tests/test_pyserq.py` checks that pyserq gives what the CLI gives: the
same report for the same program, overrides and seed, and the samples
`--dump` writes. CI runs it.
