#!/usr/bin/env python3
"""The size of the language, written to tools/metrics.json (#142).

    scripts/metrics.py           # regenerate tools/metrics.json
    scripts/metrics.py --check   # fail if it is not current (make check)
    scripts/metrics.py --report  # also print the spec's and programs' size

The file is committed like the golden files, so a change that grows the
language shows the growth in its diff: the IR's variants, the keywords,
functions and context variables a reader has to know, and the code lines
three or more programs repeat (a definition waiting to be written). The
spec's and the programs' length move with every edit and are reported, not
committed.
"""
import json
import re
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "tools" / "metrics.json"


def read(rel):
    return (ROOT / rel).read_text(encoding="utf-8")


def enum_variants(src, name):
    head = f"pub enum {name} {{"
    assert head in src, f"src/ir.rs: `{head}` not found"
    body = src[src.index(head):]
    body = body[: body.index("\n}\n")]
    return len(re.findall(r"^    [A-Z][A-Za-z]*", body, re.M))


def array_len(src, name):
    m = re.search(rf"pub const {name}: \[[^;]+; (\d+)\]", src)
    assert m, f"`pub const {name}: [...; N]` not found"
    return int(m.group(1))


def metrics():
    parser = read("src/frontend/parser.rs")
    link = read("src/frontend/link.rs")
    ir = read("src/ir.rs")
    lang = read("docs/language.md")
    programs = sorted(
        str(p.relative_to(ROOT))
        for p in list((ROOT / "examples").glob("*/*.sq")) + list((ROOT / "lib").glob("*.sq"))
    )
    code = {}
    lines_of = {}
    for p in programs:
        lines = [l.strip() for l in read(p).splitlines()]
        c = [l for l in lines if l and not l.startswith("//")]
        code[p] = len(c)
        # the code of a line, its comment stripped, long enough to be a statement
        lines_of[p] = {s for s in (l.split("//")[0].strip() for l in c) if len(s) > 30}
    seen = Counter(l for s in lines_of.values() for l in s)
    clones = sorted(l for l, n in seen.items() if n >= 3 and not l.startswith("let "))
    return {
        "IR_VERSION": int(re.search(r"IR_VERSION: u32 = (\d+)", ir).group(1)),
        "ir": {
            "CStmt": enum_variants(ir, "CStmt"),
            "CExpr": enum_variants(ir, "CExpr"),
            "Fun": enum_variants(ir, "Fun"),
            "CtxVar": enum_variants(ir, "CtxVar"),
        },
        "surface": {
            "keywords": array_len(parser, "KEYWORDS"),
            "functions": array_len(link, "FUNCTIONS") + array_len(link, "FOLDED") + array_len(link, "AGGREGATES"),
            "context_variables": array_len(link, "CONTEXT_VARS"),
        },
        "clones": clones,
    }, {
        "spec": {"language.md lines": lang.count("\n"), "language.md words": len(lang.split())},
        "programs": {"count": len(programs), "code lines": sum(code.values()), "per program": code},
    }


def main():
    committed, report = metrics()
    text = json.dumps(committed, indent=2, ensure_ascii=False) + "\n"
    if "--report" in sys.argv:
        print(json.dumps(report, indent=2, ensure_ascii=False))
    if "--check" in sys.argv:
        have = OUT.read_text() if OUT.exists() else ""
        if have != text:
            sys.stderr.write(
                "tools/metrics.json is not current: the language's size moved.\n"
                "Run scripts/metrics.py and commit the change, saying in the PR why it grew.\n"
            )
            import difflib

            sys.stderr.writelines(difflib.unified_diff(have.splitlines(True), text.splitlines(True), "committed", "now"))
            sys.exit(1)
        print(f"OK: tools/metrics.json is current ({json.loads(text)['surface']['keywords']} keywords)")
    else:
        OUT.write_text(text)
        print(f"wrote {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
