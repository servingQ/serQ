# pyserq

serQ in Python. serQ is a language in which an LLM serving deployment is a
program; pyserq compiles a program to its IR and runs it, in process, and
gives what the `serq` CLI gives.

```python
import pyserq
p = pyserq.compile("mg1.sq", sets={"lam": 0.8}, seed=10)
r = pyserq.run(p)            # the GIL is released while it runs
r.json()                     # what `serq run --json` prints
o = r.observe("sojourn")     # o.mean, o.ci, o.samples, o.times
pyserq.draw("mg1.sq", format="svg")  # what `serq draw --format svg` prints
```

```bash
pip install pyserq           # a release
pip install --pre pyserq     # the latest commit on main (X.Y.Z.devN)
```

Wheels: linux x86_64 (manylinux), macOS arm64; Python 3.9 and later (abi3).
The version is serq's: `pyserq.__version__` is the crate it was built from.
