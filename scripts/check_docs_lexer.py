#!/usr/bin/env python3
"""Every serQ program the site shows lexes without an error token.

    scripts/check_docs_lexer.py     # needs pygments (docs/requirements.txt)

A character no rule of `docs/hooks/serq_lexer.py` matches becomes a Pygments
`Error` token, which the page draws boxed in red: `E[j].decode` did, 38 times,
until `.` was punctuation. This lexes every `.sq` file the site can include
(`examples/`, `lib/`, `docs/`) and every ```serq fence under `docs/` and in
the README. `tests/docs_lexer.rs` checks the words; this checks the
characters.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docs" / "hooks"))

from pygments.token import Error  # noqa: E402
from serq_lexer import SerqLexer  # noqa: E402

FENCE = re.compile(r"^([ \t]*)```serq[^\n]*\n(.*?)^\1```", re.M | re.S)


def sources():
    for d in ("examples", "lib", "docs"):
        for p in sorted((ROOT / d).rglob("*.sq")):
            yield p, p.read_text()
    for p in [ROOT / "README.md", *sorted((ROOT / "docs").rglob("*.md"))]:
        for i, m in enumerate(FENCE.finditer(p.read_text())):
            yield f"{p} (serq block {i + 1})", m.group(2)


def main():
    lexer = SerqLexer()
    bad = []
    count = 0
    for where, text in sources():
        count += 1
        for pos, tok, val in lexer.get_tokens_unprocessed(text):
            if tok in Error:
                line = text.count("\n", 0, pos) + 1
                bad.append(f"{where}:{line}: {val!r}")
    for b in bad:
        print(b)
    if bad:
        sys.exit(f"check_docs_lexer: {len(bad)} error tokens in {count} programs")
    print(f"check_docs_lexer: {count} programs, no error token")


if __name__ == "__main__":
    main()
