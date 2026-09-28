#!/usr/bin/env python3
"""Check the `file.py:lines` citations of upstream vLLM against `ref/vllm`.

`docs/language.md` §7 asserts a correspondence between vLLM's scheduler and
`programs/vllm.seq`, and the evidence for each row is a citation into upstream
source. Nothing checked them, so a citation could name a range that had moved,
or a file that no longer existed, and the table would still read as proof.

This checks two things:

  * every cited file exists at the pinned revision and every range is in
    bounds -- cheap, and catches a deleted file or a truncated one;
  * the *text* of every cited range still hashes to what it hashed to when the
    citation was written (`tools/citations.json`). Line numbers move for
    reasons that have nothing to do with semantics -- upstream's 2026-09-16
    formatting sweep moved them across all three of the most-cited files -- so
    "the range is in bounds" is much weaker than "the range still says what it
    said".

A changed hash is not a failure of seQ. It means an upstream range moved and
the citation has to be re-read and re-pointed, which is exactly the work the
table exists to make possible.

Usage:  scripts/check_citations.py [--bless | --paths]
"""

import hashlib
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
REF = ROOT / "ref" / "vllm"
MANIFEST = ROOT / "tools" / "citations.json"

# Citations are written with the bare file name, which is how a reader of the
# prose wants them. The full path is declared once, here, so that a bare name
# that is not in this table is an error rather than a guess.
PATHS = {
    "scheduler.py": "vllm/v1/core/sched/scheduler.py",
    "request_queue.py": "vllm/v1/core/sched/request_queue.py",
    "kv_cache_manager.py": "vllm/v1/core/kv_cache_manager.py",
    "single_type_kv_cache_manager.py": "vllm/v1/core/single_type_kv_cache_manager.py",
    "block_pool.py": "vllm/v1/core/block_pool.py",
    "config/scheduler.py": "vllm/config/scheduler.py",
    "request.py": "vllm/v1/request.py",
}

SOURCES = ["docs/**/*.md", "src/**/*.rs", "tests/**/*.rs", "programs/*.seq", "README.md"]

# `scheduler.py:742-813, 869, 1539-1582` -- one file, then a list of ranges.
CITE = re.compile(r"\b((?:[\w.]+/)?[\w.]+\.py):((?:\s*\d+(?:-\d+)?\s*,)*\s*\d+(?:-\d+)?)")
RANGE = re.compile(r"(\d+)(?:-(\d+))?")


def sources():
    for pat in SOURCES:
        for p in sorted(ROOT.glob(pat)):
            if REF in p.parents:
                continue
            yield p


def citations():
    """Every citation in the repository, as (site, file, (lo, hi))."""
    out = []
    for path in sources():
        text = path.read_text(encoding="utf-8", errors="replace")
        for line_no, line in enumerate(text.splitlines(), 1):
            for m in CITE.finditer(line):
                name = m.group(1)
                site = f"{path.relative_to(ROOT)}:{line_no}"
                for r in RANGE.finditer(m.group(2)):
                    lo = int(r.group(1))
                    hi = int(r.group(2)) if r.group(2) else lo
                    out.append((site, name, lo, hi))
    return out


def digest(name, lo, hi, cache):
    """The sha256 of a cited range's text, or a reason it has none."""
    rel = PATHS.get(name)
    if rel is None:
        return None, f"no path declared for `{name}` -- add it to PATHS"
    if rel not in cache:
        f = REF / rel
        cache[rel] = f.read_text(encoding="utf-8").splitlines() if f.exists() else None
    lines = cache[rel]
    if lines is None:
        return None, f"{rel} does not exist at the pinned revision"
    if hi > len(lines):
        return None, f"{rel}:{lo}-{hi} is past the end of the file ({len(lines)} lines)"
    body = "\n".join(lines[lo - 1 : hi])
    return hashlib.sha256(body.encode()).hexdigest()[:16], None


def main():
    if "--paths" in sys.argv:
        # What a checkout has to contain for this script to run. The fetch
        # script reads it, so adding a citation to a new file widens the
        # checkout without anyone remembering to.
        print("\n".join(sorted({p for p in PATHS.values()})))
        return 0
    bless = "--bless" in sys.argv
    if not REF.exists():
        print(
            "SKIP: ref/vllm is absent -- run scripts/fetch_vllm_ref.sh to check citations",
            file=sys.stderr,
        )
        return 0

    found = citations()
    if not found:
        print("FAIL: no citations found; the pattern or the source list is wrong")
        return 1

    want = json.loads(MANIFEST.read_text()) if MANIFEST.exists() else {}
    have, errors, moved = {}, [], []
    cache = {}
    for site, name, lo, hi in found:
        key = f"{name}:{lo}-{hi}" if hi != lo else f"{name}:{lo}"
        h, err = digest(name, lo, hi, cache)
        if err:
            errors.append(f"{site}: {err}")
            continue
        have[key] = h
        if key in want and want[key] != h and not bless:
            moved.append(f"{site}: {key} no longer says what it said")

    if bless:
        MANIFEST.write_text(json.dumps(dict(sorted(have.items())), indent=1) + "\n")
        print(f"blessed {len(have)} citations into {MANIFEST.relative_to(ROOT)}")
        return 0

    for e in errors + moved:
        print(f"FAIL: {e}")
    new = sorted(set(have) - set(want))
    if new:
        print(f"FAIL: {len(new)} citation(s) not in the manifest; run `make citations`")
        for k in new[:10]:
            print(f"  {k}")
    if errors or moved or new:
        return 1
    print(f"OK: {len(have)} citations resolve at the pinned revision")
    return 0


if __name__ == "__main__":
    sys.exit(main())
