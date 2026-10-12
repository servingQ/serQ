#!/usr/bin/env python3
"""The IR variants no program uses, written to tools/unused.json.

    scripts/unused.py           # regenerate tools/unused.json
    scripts/unused.py --check   # fail if it is not current (make check)

A construct that only tests reach is a path no reader of the examples ever
sees (CLAUDE.md, §Keeping the frontend small). The programs are every
example and tutorial program, compiled with `serq ir`, and the IR the
oracles and the Lean claims read (`tools/**/*.ir.json`); the variants are
those of each enum of `src/ir.rs` that is serialised, read from the source.
A variant counts as used when its name is a tag anywhere in a program's
IR, so a name two enums share (`Exp`) is used when either is: the list can
miss an unused variant, never name a used one. The file is committed, so a
change that leaves a construct without a program, or gives one a program,
shows it in its diff.
"""
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "tools" / "unused.json"
SERQ = ROOT / "target" / "release" / "serq"


def variants():
    """Each serialised enum of the IR and its variants, in source order."""
    src = (ROOT / "src" / "ir.rs").read_text()
    out = {}
    for m in re.finditer(r"#\[derive\(([^)]*)\)\]\s*pub enum (\w+)\b[^{]*\{(.*?)\n\}", src, re.S):
        derive, name, body = m.groups()
        if "Serialize" in derive:
            out[name] = re.findall(r"^    ([A-Z][A-Za-z0-9]*)\b", body, re.M)
    return out


def programs():
    examples = [p for p in (ROOT / "examples").rglob("*.sq") if "instances" not in p.parts]
    return sorted(examples) + sorted((ROOT / "docs" / "tutorial" / "programs").glob("*.sq"))


def tags(x, out):
    """Every enum tag in a serde JSON value: an object's keys, and strings."""
    if isinstance(x, dict):
        for k, v in x.items():
            out.add(k)
            tags(v, out)
    elif isinstance(x, list):
        for v in x:
            tags(v, out)
    elif isinstance(x, str):
        out.add(x)


def unused():
    used = set()
    for p in programs():
        ir = subprocess.run([str(SERQ), "ir", str(p), "--horizon", "10"],
                            capture_output=True, text=True, check=True, cwd=ROOT)
        tags(json.loads(ir.stdout), used)
    # the IR the oracles and the Lean claims read: an example with its
    # scenario's sessions written in
    for p in sorted((ROOT / "tools").glob("**/*.ir.json")):
        tags(json.loads(p.read_text()), used)
    out = {}
    for name, vs in variants().items():
        left = [v for v in vs if v not in used]
        if left:
            out[name] = left
    return out


def main():
    current = json.dumps(unused(), indent=2) + "\n"
    if "--check" in sys.argv:
        if not OUT.exists() or OUT.read_text() != current:
            print(f"FAIL: {OUT.relative_to(ROOT)} is not current: run `make unused` and say in the PR "
                  "why a construct has no program, or which program it gained")
            print(current)
            sys.exit(1)
        print(f"OK: {OUT.relative_to(ROOT)} is current")
    else:
        OUT.write_text(current)
        print(current, end="")


if __name__ == "__main__":
    main()
