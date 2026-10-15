#!/usr/bin/env python3
"""The IR variants no program uses, written to tools/unused.json.

    scripts/unused.py           # regenerate tools/unused.json
    scripts/unused.py --check   # fail if it is not current (make check)

A construct that only tests reach is a path no reader of the examples ever
sees (CLAUDE.md, §Keeping the frontend small). The programs are every
example and tutorial program, compiled with `serq ir` with its workload
(examples/pairs.json), and the IR the
oracles and the Lean claims read (`tools/**/*.ir.json`); a companion
repository's programs are not read here. The variants are those of each
enum of `src/ir.rs` that is serialised, read from the source.

A variant counts as used when its name is a tag anywhere in a program's
IR. A name two enums share (`Exp` is a function and a distribution) is
used when either is, so it may hide an unused variant; `shared` lists
those names. The list never names a used variant: serde writes a variant
under its own name, which `variants` checks the source still does.
"""
import json
import re
import subprocess
import sys
from pathlib import Path

import pairs

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "tools" / "unused.json"
SERQ = ROOT / "target" / "release" / "serq"


def variants():
    """Each serialised enum of the IR and its variants, in source order."""
    src = (ROOT / "src" / "ir.rs").read_text(encoding="utf-8")
    # a variant renamed or untagged is no longer a tag of its own name
    public = src[: src.index("mod real")] + src[src.index("mod real") :].split("\n}\n", 1)[1]
    assert not re.search(r"#\[serde\((rename|tag|untagged|content)", public), \
        "src/ir.rs renames or untags a variant: read the tag, not the name"
    out = {}
    for m in re.finditer(r"#\[derive\(([^)]*)\)\]\s*pub enum (\w+)\b[^{]*\{(.*?)\n\}", src, re.S):
        derive, name, body = m.groups()
        if "Serialize" in derive:
            out[name] = re.findall(r"^    ([A-Z][A-Za-z0-9]*)\b", body, re.M)
    serialised = len(re.findall(r"#\[derive\([^)]*Serialize[^)]*\)\]\s*pub enum ", src))
    assert len(out) == serialised, "an attribute between a derive and its enum: read it"
    return out


def programs():
    """The programs `make check` links and draws, and the tutorial's, each
    run with its workload (examples/pairs.json)."""
    return pairs.programs()


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
    # each program under each workload it runs (examples/pairs.json)
    for p in programs():
        for w in pairs.workloads(p) or [None]:
            args = ["--workload", str(w)] if w else []
            ir = subprocess.run([str(SERQ), "ir", str(p), *args, "--horizon", "10"], capture_output=True, text=True, cwd=ROOT)
            if ir.returncode:
                sys.exit(f"FAIL: {p.relative_to(ROOT)}: serq ir failed\n{ir.stderr}")
            tags(json.loads(ir.stdout), used)
    # the IR the oracles and the Lean claims read: an example with its
    # scenario's sessions written in
    for p in sorted((ROOT / "tools").glob("**/*.ir.json")):
        tags(json.loads(p.read_text(encoding="utf-8")), used)
    enums = variants()
    owners = {}
    for name, vs in enums.items():
        for v in vs:
            owners.setdefault(v, []).append(name)
    out = {}
    for name, vs in enums.items():
        left = [v for v in vs if v not in used]
        if left:
            out[name] = left
    out["shared"] = sorted(v for v, o in owners.items() if len(o) > 1 and v in used)
    return out


def main():
    current = json.dumps(unused(), indent=2) + "\n"
    if "--check" in sys.argv:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != current:
            print(f"FAIL: {OUT.relative_to(ROOT)} is not current: run `make unused` and say in the PR "
                  "why a construct has no program, or which program it gained")
            print(current)
            sys.exit(1)
        print(f"OK: {OUT.relative_to(ROOT)} is current")
    else:
        OUT.write_text(current, encoding="utf-8")
        print(current, end="")


if __name__ == "__main__":
    main()
