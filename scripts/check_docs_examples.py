#!/usr/bin/env python3
"""The site's example programs link and run.

    scripts/check_docs_examples.py            # the ```serq fences (make check); needs target/release/serq
    scripts/check_docs_examples.py --python   # the ```python fences (CI's pyserq job); needs pyserq installed

A page shows a complete program as two ```serq fences titled "model.sq" and
"workload.sq", the names it tells the reader to save them under. This saves
them under those names in a temporary directory, links them (`serq check
model.sq --workload workload.sq`) and runs there every `serq` line of the
page's ```sh fences that names `model.sq`, as `serq` with its arguments (no
shell). Any other ```serq fence that declares `fn main` (not one that names
it in a comment) shows part of a program, and says so:
```serq { .excerpt }. The marker on a fence the check would link anyway, or
on one with no `fn main`, is refused, so the marked fences are exactly the
ones the check leaves out.

A ```python fence that imports pyserq is a script, and runs in a temporary
directory with the Python running this.

A fence with neither is a fragment: `scripts/check_docs_lexer.py` lexes it.
A fragment worth linking is included from a program `make check` links, by a
named snippet section (`--8<-- "path.sq:section"`, docs/writing-programs.md).
A fence that includes a file (`--8<--`) is that file, which `make check`
links where it lives. Pages mkdocs.yml leaves out of the site
(`exclude_docs`: the design records) are not read: a design record's Before
is the code as it was.

SERQ_CLI names the serq binary, as for pyserq/tests/test_pyserq.py.
"""

import os
import re
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SERQ = os.environ.get("SERQ_CLI", str(ROOT / "target" / "release" / "serq"))
OPEN = re.compile(r"^([ \t]*)(`{3,}|~{3,})(.*)$")
BRACES = re.compile(r"\{([^}]*)\}")
CLASS = re.compile(r"(?<![\w-])\.([\w-]+)")
TITLE = re.compile(r'\btitle="([^"]*)"')
MAIN = re.compile(r"\bfn\s+main\s*\(")
COMMENT = re.compile(r"/\*.*?\*/|//[^\n]*", re.S)
PYSERQ = re.compile(r"^\s*import pyserq\b", re.M)
EXCERPT = "{ .excerpt }"
PAIR = ("model.sq", "workload.sq")
SHELL = ("sh", "bash", "shell")


def declares_main(body):
    """The fence declares `fn main`, not merely names it in a comment (`// in fn main()`)."""
    return bool(MAIN.search(COMMENT.sub("", body)))


def excluded(mkdocs_text):
    """mkdocs.yml's `exclude_docs: |` block: `/dir/` and `/page.md` patterns under docs/."""
    lines = mkdocs_text.splitlines()
    if "exclude_docs: |" not in lines:
        return []
    out = []
    for line in lines[lines.index("exclude_docs: |") + 1:]:
        if line and not line.startswith(" "):
            break
        pattern = line.strip()
        if not pattern:
            continue
        if not pattern.startswith("/") or any(c in pattern for c in "*?[!"):
            # a pattern read wrongly would check pages the site leaves out, or skip ones it builds
            sys.exit(f"check_docs_examples: mkdocs.yml excludes {pattern!r}; this reads only "
                     "`/dir/` and `/page.md` patterns: teach excluded() the new form")
        out.append(pattern.lstrip("/"))
    return out


def pages(root=ROOT):
    """The Markdown pages the site builds from docs/."""
    patterns = excluded((root / "mkdocs.yml").read_text())
    for p in sorted((root / "docs").rglob("*.md")):
        rel = p.relative_to(root / "docs").as_posix()
        if not any(rel == pat or (pat.endswith("/") and rel.startswith(pat)) for pat in patterns):
            yield p


def info_parts(info):
    """(language, classes, title) of a fence's info string.

    `serq title="model.sq"`, `serq { .excerpt }` and `{ .serq .excerpt }` are
    all spellings superfences reads, so the language of the brace form is its
    first class: a fence cannot leave the check by how it is spelt.
    """
    classes = [c for b in BRACES.findall(info) for c in CLASS.findall(b)]
    words = info.split()
    if words and not words[0].startswith("{"):
        lang = words[0]
    else:
        lang = classes.pop(0) if classes else ""
    t = TITLE.search(info)
    return lang, classes, t.group(1) if t else None


def fences(path):
    """(line of the opening fence, info string, body) of each fence, indented ones included."""
    lines = path.read_text().splitlines()
    i = 0
    while i < len(lines):
        m = OPEN.match(lines[i])
        if not m or (m.group(2)[0] == "`" and "`" in m.group(3)):
            i += 1
            continue
        indent, mark, info = m.groups()
        close = re.compile(rf"^[ \t]*{re.escape(mark[0])}{{{len(mark)},}}[ \t]*$")
        j = i + 1
        while j < len(lines) and not close.match(lines[j]):
            j += 1
        body = "\n".join(l[len(indent):] if l.startswith(indent) else l for l in lines[i + 1:j])
        yield i + 1, info.strip(), body + "\n"
        i = j + 1


def serq(*args, cwd):
    r = subprocess.run([SERQ, *args], cwd=cwd, capture_output=True, text=True)
    return r.returncode, (r.stdout + r.stderr).strip()


def commands(body):
    """(line in the fence, the command, its words) of each `serq` line of a shell fence; `\\` continues a line."""
    lines = body.splitlines()
    k = 0
    while k < len(lines):
        start, cmd = k + 1, lines[k]
        while cmd.endswith("\\") and k + 1 < len(lines):
            k += 1
            cmd = cmd[:-1] + " " + lines[k]
        k += 1
        if cmd.split()[:1] == ["serq"]:
            yield start, cmd.strip(), shlex.split(cmd, comments=True)


def check_serq(page, rel):
    """Errors of one page's ```serq fences, and the pairs linked and commands run."""
    errors, linked, ran = [], 0, 0
    fs = [(line, *info_parts(info), body) for line, info, body in fences(page)]
    titled = {}
    for line, lang, classes, name, body in fs:
        if lang != "serq" or "--8<--" in body:
            continue
        if name in PAIR:
            titled.setdefault(name, []).append((line, body))
        if "excerpt" in classes:
            if not declares_main(body) or name in PAIR:
                errors.append(f"{rel}:{line}: `{EXCERPT}` marks a fence the check would otherwise link, "
                              "one with `fn main` and no model.sq title; this one is not: drop the marker")
        elif declares_main(body) and name != "model.sq":
            errors.append(f"{rel}:{line}: a ```serq fence with `fn main` is a program a reader can save: "
                          'title it "model.sq" beside the page\'s "workload.sq" fence, which make check links, '
                          f"or, if it shows part of a program, mark it ```serq {EXCERPT}")
    if not titled:
        return errors, linked, ran
    for name in PAIR:
        found = titled.get(name, [])
        if len(found) != 1:
            other = PAIR[1 - PAIR.index(name)]
            errors.append(f"{rel}: {len(found)} fences titled {name} beside {other}: a page that tells the reader "
                          f"to save a program shows one {PAIR[0]} and one {PAIR[1]}, the deployment and its workload")
    if errors:
        return errors, linked, ran
    with tempfile.TemporaryDirectory() as d:
        for name, [(line, body)] in titled.items():
            (Path(d) / name).write_text(body)
        code, out = serq("check", PAIR[0], "--workload", PAIR[1], cwd=d)
        linked += 1
        if code:
            line = titled[PAIR[0]][0][0]
            errors.append(f"{rel}:{line}: the page's model.sq with its workload.sq does not link "
                          f"(lines are the fence's):\n{out}")
            return errors, linked, ran
        for line, lang, _, _, body in fs:
            if lang not in SHELL:
                continue
            for k, cmd, words in commands(body):
                if PAIR[0] not in words:
                    continue
                code, out = serq(*words[1:], cwd=d)
                ran += 1
                if code:
                    errors.append(f"{rel}:{line + k}: `{cmd}` fails on the page's model.sq and workload.sq:\n{out}")
    return errors, linked, ran


def run_python(body, where):
    """Run one script in a temporary directory; an error names its page line."""
    with tempfile.TemporaryDirectory() as d:
        script = Path(d) / "example.py"
        script.write_text(body)
        r = subprocess.run([sys.executable, str(script)], cwd=d, capture_output=True, text=True)
    return None if r.returncode == 0 else f"{where}: the script fails:\n{(r.stdout + r.stderr).strip()}"


def main(argv):
    errors = []
    if "--python" in argv:
        try:
            import pyserq  # a missing pyserq fails here, once, not in every fence
        except ImportError as e:
            sys.exit(f"check_docs_examples --python: {e}: install the pyserq this tree builds "
                     "(maturin build -m pyserq/Cargo.toml, as ci.yml's pyserq job does)")
        n = 0
        for page in pages():
            rel = page.relative_to(ROOT)
            for line, info, body in fences(page):
                if info_parts(info)[0] == "python" and PYSERQ.search(body):
                    n += 1
                    e = run_python(body, f"{rel}:{line}")
                    if e:
                        errors.append(e)
        summary = f"{n} pyserq scripts run (pyserq {pyserq.__version__})"
    else:
        linked = ran = 0
        for page in pages():
            e, lk, r = check_serq(page, page.relative_to(ROOT))
            errors += e
            linked += lk
            ran += r
        summary = f"{linked} programs link with their workloads, {ran} commands run"
    for e in errors:
        print(e)
    if errors:
        sys.exit(f"check_docs_examples: {len(errors)} errors")
    print(f"check_docs_examples: {summary}")


if __name__ == "__main__":
    main(sys.argv[1:])
