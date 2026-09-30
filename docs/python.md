# Python: pyserq

`pyserq` runs serQ in process. It shows what the [IR](ir.md) is the
definition of: a program is compiled, from a file or from text, to its IR,
and the IR is run. Nothing of the text frontend is exposed.

```python
import pyserq

p = pyserq.compile("examples/single-turn/mg1.sq", sets={"lam": 0.8, "law": 1}, seed=10)
r = pyserq.run(p)            # the GIL is released while it runs
r.json()                     # what `serq run --json` prints
o = r.observe("sojourn")     # o.mean, o.ci, o.p99, ...
o.samples, o.times           # what `--dump` writes
r.stage("svc").utilization   # r.observes, r.stages, r.pools: all of them
pyserq.read_trace("examples/replay/data/short_base.csv")  # the sessions a replay draws from
```

| | |
|---|---|
| `compile(path=None, *, source=None, sets={}, seed, horizon, warmup, arrivals, trace)` | A program file or text to its IR, with the overrides of `serq run`. A number in `sets` is that number (an infinity is `inf`; NaN is refused); a string is an expression. A relative trace is read next to the program file for `compile(path)`, and from the current directory with `trace=`, `source=` or `Program.from_json`, as `serq run` does. |
| `Program.to_json()`, `Program.from_json(s)` | The IR as JSON (`serq ir`), and back. |
| `run(program)` | A run. Runs in threads proceed in parallel. |
| `Report.json()` | The summary `serq run --json` prints. |
| `Report.horizon`, `.end`, `.warmup`, `.seed`, `.events`, `.arrivals`, `.ended`, `.turns`, `.mean_live` | The summary's fields, by the same names. A field JSON writes as `null` is the number the run computed, which is not finite (NaN, or ±inf). |
| `Report.observes` | The observations by name (a new dict on each access, in the program's order), each an `Observe`: `name`, `count`, `mean`, `ci` (batch-means 95 % half-width, +inf below 40 samples), `cv2`, `p99` as in the summary, and its samples `samples`, `times`, `sessions`, `turns` as `--dump` writes them (each access makes a new list: bind it once). |
| `Report.stages`, `Report.pools` | One `Stage` or `Pool` per row of the summary, with its fields by the same names. |
| `Report.observe(name)`, `.stage(name)`, `.stages_named(name)`, `.pool(name)` | One by name, or `None`, as `serq::Report` has them; `stages_named` gives every member of a replicated stage. |
| `Rng(seed)` | The generator a run draws from, rand 0.9's `StdRng` seeded by `seed_from_u64`: `next_u32()`, `next_u64()`, `random_f64()` in [0, 1), and `range_u64`, `range_u32`, `range_f64(low, high)`, both ends included. A run seeded `s` draws its arrivals from `Rng(s)`; a check that reproduces a run's draws uses it rather than a port of rand. An empty range raises `ValueError`. |
| `read_trace(path)` | A trace's sessions, each a list of its turns `(new, out, think, forced)`: the corpus a replay draws its sessions from, read as a replay reads it. A relative path is read from the current directory. |
| `IR_VERSION`, `REPORT_VERSION`, `__version__` | The IR it reads, the shape of the report (the field names of `Report.json()`, which `Report`, `Observe`, `Stage` and `Pool` carry as attributes; a change bumps it), and the serq version it is. |

A program that does not compile or run raises `ValueError` with serQ's message; an argument of the wrong type (`seed=-1`, `sets={"x": None}`) raises `TypeError` or `OverflowError`, as Python does.

## Install

A release carries one wheel per platform (linux x86_64, macOS arm64), with
the version of its tag (`v0.1.0-rc7` is `pyserq 0.1.0rc7`). The repository is
private, so download the wheel with `gh release download` and install it:

```bash
gh release download v0.1.0-rc7 -R vrvrv/serQ -p 'pyserq-*.whl' -D wheels
pip install wheels/pyserq-*manylinux*.whl      # or the macosx one
```

Or build it from a checkout: `pip install maturin && maturin develop -m pyserq/Cargo.toml`.

`pyserq/tests/test_pyserq.py` checks that pyserq gives what the CLI gives: the
same report for the same program, overrides and seed, and the samples
`--dump` writes. CI runs it.
