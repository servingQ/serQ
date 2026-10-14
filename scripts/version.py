#!/usr/bin/env python3
"""serQ has one version: `[workspace.package] version` in Cargo.toml, a
release `X.Y.Z` or a pre-release `X.Y.Z-alpha.N` (`beta`, `rc`). The serq
crate and pyserq inherit it
(`version.workspace = true`), and pyserq's wheel takes it from the crate
(pyproject's `dynamic = ["version"]`), so the three cannot disagree.

    scripts/version.py check               # the layout above holds (make check, CI)
    scripts/version.py check --tag vX.Y.Z  # ... and the tag is that version (release, pre-release)
    scripts/version.py wheels DIR VERSION  # every wheel in DIR is VERSION (PEP 440)
    scripts/version.py dev                 # the dev version of HEAD, or nothing
    scripts/version.py stamp VERSION       # write a dev version into Cargo.toml (CI only)

A pre-release is tagged like a release, from a commit that sets its version,
so `serq --version`, a report's `serq_version` and the crate say which one it
is: `0.1.6-alpha.6` to Cargo, `0.1.6a6` to pip.

A dev version is numbered by the commits since the version's tag, and comes
before the next version: 3 commits after v0.1.0 is `0.1.1-dev.3` to Cargo and
`0.1.1.dev3` to pip; 3 after v0.1.6-alpha.6 is `0.1.6-alpha.7.dev.3`, pip's
`0.1.6a7.dev3`. It is stamped at build time and never committed; `dev` prints
nothing at a tagged commit or before the version's tag exists.
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
# X.Y.Z, or a pre-release of it: X.Y.Z-alpha.N, -beta.N, -rc.N
VERSION = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-(alpha|beta|rc)\.(\d+))?$")
DEV = re.compile(r"^\d+\.\d+\.\d+(?:-(?:alpha|beta|rc)\.\d+\.|-)dev\.\d+$")
PEP440 = {"-alpha.": "a", "-beta.": "b", "-rc.": "rc", ".dev.": ".dev", "-dev.": ".dev"}


def fail(msg):
    sys.exit(f"version: {msg}")


def read(path):
    return (ROOT / path).read_text()


def version():
    # the first `version = "..."` in Cargo.toml is [workspace.package]'s
    # (python3 on ubuntu-22.04 is 3.10, which has no tomllib)
    m = re.search(r'(?m)^version = "([^"]*)"', read("Cargo.toml"))
    if not m or "[workspace.package]" not in read("Cargo.toml")[: m.start()]:
        fail("Cargo.toml: no version under [workspace.package]")
    return m.group(1)


def check(tag=None):
    v = version()
    if not VERSION.match(v):
        fail(f"Cargo.toml's version is {v!r}; a version is X.Y.Z or X.Y.Z-alpha.N "
             "(beta, rc); a dev version is stamped by CI, not committed")
    for crate in ["Cargo.toml", "pyserq/Cargo.toml"]:
        package = read(crate).split("[package]", 1)[1].split("\n[", 1)[0]
        if not re.search(r"(?m)^version\.workspace = true$", package):
            fail(f"{crate}: the crate's version must be `version.workspace = true`")
    pyproject = read("pyserq/pyproject.toml")
    if re.search(r"(?m)^version\s*=", pyproject) or not re.search(
        r'(?m)^dynamic = \[[^]]*"version"', pyproject
    ):
        fail("pyserq/pyproject.toml: the version must be dynamic (the crate's), not written")
    if tag is not None and tag != f"v{v}":
        fail(f"tag {tag} but Cargo.toml says {v}")
    print(f"OK: serq and pyserq are {v}")


def pep440(v):
    # as maturin writes it: 0.1.6-alpha.7.dev.3 -> 0.1.6a7.dev3
    for semver, pep in PEP440.items():
        v = v.replace(semver, pep)
    return v


def wheels(d, v):
    found = sorted(Path(d).rglob("*.whl"))
    if not found:
        fail(f"no wheels in {d}")
    for w in found:
        got = w.name.split("-")[1]
        if got != pep440(v):
            fail(f"{w.name} is {got}, not {pep440(v)}")
    print(f"OK: {len(found)} wheels are {pep440(v)}")


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True)


def dev():
    v = version()
    m = VERSION.match(v)
    if not m:
        fail(f"Cargo.toml's version is {v!r}, not X.Y.Z or X.Y.Z-alpha.N")
    tag = f"v{v}"
    if git("rev-parse", "-q", "--verify", f"refs/tags/{tag}").returncode != 0:
        return  # the bump has merged and the tag is not pushed yet
    n = int(git("rev-list", "--count", f"{tag}..HEAD").stdout)
    if n == 0:
        return  # the release itself
    x, y, z, pre, k = m.groups()
    if pre:
        print(f"{x}.{y}.{z}-{pre}.{int(k) + 1}.dev.{n}")
    else:
        print(f"{x}.{y}.{int(z) + 1}-dev.{n}")


def stamp(v):
    if not DEV.match(v):
        fail(f"{v!r} is not a dev version X.Y.Z-dev.N or X.Y.Z-alpha.N.dev.M")
    p = ROOT / "Cargo.toml"
    text, n = re.subn(r'(?m)^version = "[^"]*"', f'version = "{v}"', p.read_text(), count=1)
    if n != 1:
        fail("no version line in Cargo.toml")
    p.write_text(text)
    print(f"stamped {v}")


if __name__ == "__main__":
    a = sys.argv[1:]
    match a:
        case ["check"]:
            check()
        case ["check", "--tag", t]:
            check(t)
        case ["wheels", d, v]:
            wheels(d, v)
        case ["dev"]:
            dev()
        case ["stamp", v]:
            stamp(v)
        case _:
            sys.exit(__doc__)
