#!/usr/bin/env python3
"""The workload each program runs with: examples/pairs.json.

A program is a deployment; its workload is a file of its own, given when it
runs (`serq run D --workload W`, docs/design/paired-workload.md).
`examples/pairs.json` maps each example and tutorial program to its
workload, both paths relative to the repository's root.

    scripts/pairs.py --check   # every program has a workload; every path exists
    scripts/pairs.py FILE      # print FILE's workload (nothing when it has none)

The tools that run a program import `workload_args` from here.
"""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PAIRS = ROOT / "examples" / "pairs.json"
# where the workloads live: files of a pair, never programs run alone
WORKLOAD_DIRS = (
    ROOT / "examples" / "workloads",
    ROOT / "docs" / "tutorial" / "programs" / "workloads",
    ROOT / "tests" / "lean-regress" / "workloads",
)


def load():
    return json.loads(PAIRS.read_text(encoding="utf-8"))


def rel(path):
    return Path(path).resolve().relative_to(ROOT).as_posix()


def workload(path):
    """The workload file of the program at `path` (absolute), or None."""
    try:
        w = load().get(rel(path))
    except ValueError:  # outside the repository
        return None
    return ROOT / w if w else None


def workload_args(path):
    """`["--workload", W]` for the program at `path`, or `[]` when it has none."""
    w = workload(path)
    return ["--workload", str(w)] if w else []


def programs():
    """The example and tutorial programs (deployments), the workloads left out."""
    ex = [p for p in sorted((ROOT / "examples").glob("*/*.sq")) if p.parent not in WORKLOAD_DIRS]
    return ex + sorted((ROOT / "docs" / "tutorial" / "programs").glob("*.sq"))


def problems():
    pairs = load()
    out = []
    for d, w in pairs.items():
        for p in (d, w):
            if not (ROOT / p).is_file():
                out.append(f"examples/pairs.json: {p} does not exist")
        if (ROOT / d).parent in WORKLOAD_DIRS:
            out.append(f"examples/pairs.json: {d} is a workload, not a program")
        if (ROOT / w).parent not in WORKLOAD_DIRS:
            out.append(f"examples/pairs.json: {w} is not under a workloads/ directory")
    for p in programs():
        if rel(p) not in pairs:
            out.append(f"{rel(p)}: no workload in examples/pairs.json")
    used = set(pairs.values())
    for d in WORKLOAD_DIRS:
        for w in sorted(d.glob("*.sq")):
            if rel(w) not in used:
                out.append(f"{rel(w)}: no program in examples/pairs.json runs it")
    return out


def main():
    if sys.argv[1:] == ["--check"]:
        bad = problems()
        for b in bad:
            print("FAIL:", b)
        if bad:
            sys.exit(1)
        print(f"OK: {len(load())} programs paired with their workloads (examples/pairs.json)")
    elif len(sys.argv) == 2:
        w = workload(Path(sys.argv[1]))
        if w:
            print(w.relative_to(ROOT).as_posix())
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
