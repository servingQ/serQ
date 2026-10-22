#!/usr/bin/env python3
"""Every serQ program the site shows lexes without an error token.

    scripts/check_docs_lexer.py     # needs pygments (docs/requirements.txt)

A character no rule of `docs/hooks/serq_lexer.py` matches becomes a Pygments
`Error` token, which the page draws boxed in red: `E[j].decode` did, 38 times,
until `.` was punctuation. This lexes every `.sq` file the site can include
(`examples/`, `docs/`) and every fence under `docs/` and in the README
that the site draws as serQ, read as `scripts/check_docs_examples.py`
reads them (```serq, ```seq, ```{ .serq }, …). `tests/docs_lexer.rs`
checks the words; this checks the characters.
"""

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docs" / "hooks"))
sys.path.insert(0, str(ROOT / "scripts"))

import check_docs_examples as examples  # noqa: E402
from pygments.token import Error  # noqa: E402
from serq_lexer import SerqLexer  # noqa: E402


def sources():
    """(where, the line before its first, text) of each program."""
    for d in ("examples", "docs"):
        for p in sorted((ROOT / d).rglob("*.sq")):
            yield p, 0, p.read_text()
    for p in [ROOT / "README.md", *sorted((ROOT / "docs").rglob("*.md"))]:
        for line, info, body in examples.fences(p):
            if examples.info_parts(info)[0] in examples.SERQ_LANGS:
                yield p, line, body


def main():
    if examples.SERQ_LANGS != set(SerqLexer.aliases):
        # check_docs_examples reads the aliases from the source, without Pygments
        sys.exit(f"check_docs_lexer: check_docs_examples reads {sorted(examples.SERQ_LANGS)} as SerqLexer's "
                 f"aliases, which are {SerqLexer.aliases}: teach serq_languages() how they are written")
    lexer = SerqLexer()
    bad = []
    count = 0
    for where, before, text in sources():
        count += 1
        for pos, tok, val in lexer.get_tokens_unprocessed(text):
            if tok in Error:
                line = before + text.count("\n", 0, pos) + 1
                bad.append(f"{where}:{line}: {val!r}")
    for b in bad:
        print(b)
    if bad:
        sys.exit(f"check_docs_lexer: {len(bad)} error tokens in {count} programs")
    print(f"check_docs_lexer: {count} programs, no error token")


if __name__ == "__main__":
    main()
