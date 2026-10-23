"""scripts/check_docs_examples.py refuses what it says it refuses.

Run from the repository root: python3 scripts/test_check_docs_examples.py
(needs target/release/serq; the pyserq case runs only where PYSERQ_FROM_TREE
is set, as CI's pyserq job sets it beside the wheel it builds from this tree,
and is skipped, by name, elsewhere)
"""
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_docs_examples as docs  # noqa: E402

FENCE = "```"
MODEL = """fn main() {
  pool reqs { cap 2; }
  stage svc : fifo;
  server {
    hold reqs (COST) { run svc (cost(svc, 2)); }
  }
}
"""
WORKLOAD = "workload {\n  arrive batch(4);\n}\n"


def pair(cost="cost(reqs, 1)", run="serq run model.sq --workload workload.sq --horizon 10"):
    return (f'{FENCE}serq title="model.sq"\n{MODEL.replace("COST", cost)}{FENCE}\n\n'
            f'{FENCE}serq title="workload.sq"\n{WORKLOAD}{FENCE}\n\n'
            f"{FENCE}sh\n{run}\n{FENCE}\n")


def check(text):
    with tempfile.TemporaryDirectory() as d:
        p = Path(d) / "page.md"
        p.write_text(text)
        return docs.check_serq(p, "page.md")


class Pages(unittest.TestCase):
    def test_the_site_leaves_out_what_mkdocs_excludes(self):
        self.assertEqual(docs.excluded("site_name: x\nexclude_docs: |\n  /design/\n  /review.md\n\nnav:\n"),
                         ["design/", "review.md"])
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "docs" / "design").mkdir(parents=True)
            (root / "mkdocs.yml").write_text("exclude_docs: |\n  /design/\n")
            (root / "docs" / "a.md").write_text("")
            (root / "docs" / "design" / "b.md").write_text("")
            self.assertEqual([p.name for p in docs.pages(root)], ["a.md"])

    def test_the_serq_languages_are_the_lexer_aliases(self):
        with tempfile.TemporaryDirectory() as d:
            lexer = Path(d) / "serq_lexer.py"
            lexer.write_text('class SerqLexer(RegexLexer):\n    name = "serQ"\n    aliases = ["serq", "sq"]\n')
            self.assertEqual(docs.serq_languages(lexer), {"serq", "sq"})
            # a list it cannot read stops the check rather than leave every fence out
            lexer.write_text('class SerqLexer(RegexLexer):\n    aliases = ALIASES\n')
            with self.assertRaises(SystemExit):
                docs.serq_languages(lexer)

    def test_an_exclusion_it_cannot_read_stops_the_check(self):
        # a glob read as a prefix would check pages the site leaves out, or skip ones it builds
        with self.assertRaises(SystemExit) as e:
            docs.excluded("exclude_docs: |\n  *.draft.md\n")
        self.assertIn("teach excluded()", str(e.exception))


class Serq(unittest.TestCase):
    def test_a_pair_links_and_the_page_command_runs(self):
        self.assertEqual(check(pair()), ([], 1, 1))

    def test_a_bare_number_cost_does_not_link(self):
        # #497 as it was found: the pyserq pages' programs held `hold reqs (1)` after #390
        errors, _, _ = check(pair(cost="1"))
        self.assertEqual(len(errors), 1)
        self.assertTrue(errors[0].startswith("page.md:1:"), errors[0])
        self.assertIn("does not link", errors[0])
        self.assertIn("cost type", errors[0])

    def test_a_command_that_fails_names_its_line(self):
        errors, _, _ = check(pair(run="serq run model.sq --workload workload.sq"))
        self.assertEqual(len(errors), 1)
        # line 17 is the sh fence (1 + 9 + 1 + 5 + 1), so its first command is line 18
        self.assertTrue(errors[0].startswith("page.md:18:"), errors[0])
        self.assertIn("no horizon", errors[0])

    def test_a_continued_command_runs_whole(self):
        self.assertEqual(check(pair(run="serq run model.sq \\\n  --workload workload.sq --horizon 10")),
                         ([], 1, 1))

    def test_a_main_with_no_ellipsis_is_refused(self):
        errors, _, _ = check(f"{FENCE}serq\n{MODEL}{FENCE}\n")
        self.assertEqual(len(errors), 1)
        self.assertIn("write `…`", errors[0])

    def test_every_spelling_the_site_draws_as_serq_is_read(self):
        # superfences draws all of these with SerqLexer (`seq` is its other alias)
        for info in ("seq", ".serq", "SERQ", "{ .serq }", '{ .seq title="a.sq" }'):
            with self.subTest(info=info):
                errors, _, _ = check(f"{FENCE}{info}\n{MODEL}{FENCE}\n")
                self.assertEqual(len(errors), 1)
                self.assertIn("write `…`", errors[0])
        # a title read in the brace form links like any other
        text = pair().replace('serq title="model.sq"', '{ .serq title="model.sq" }')
        self.assertEqual(check(text), ([], 1, 1))

    def test_an_excerpt_is_not_linked(self):
        # serq refuses `…`, so the fence cannot be a program the reader saves
        self.assertEqual(check(f"{FENCE}serq\nfn main() {{\n  server {{ … }}\n}}\n{FENCE}\n"), ([], 0, 0))

    def test_an_ellipsis_in_a_comment_or_a_string_leaves_nothing_out(self):
        for elsewhere in ('// the rest …\n', 'let s = "…";\n'):
            with self.subTest(elsewhere=elsewhere):
                errors, _, _ = check(f"{FENCE}serq\n{elsewhere}{MODEL}{FENCE}\n")
                self.assertEqual(len(errors), 1)
                self.assertIn("write `…`", errors[0])

    def test_a_main_named_in_a_comment_is_a_fragment(self):
        self.assertEqual(check(f"{FENCE}serq\n// in fn main()\nserver {{ … }}\n{FENCE}\n"), ([], 0, 0))

    def test_a_model_needs_its_workload(self):
        errors, _, _ = check(f'{FENCE}serq title="model.sq"\n{MODEL}{FENCE}\n')
        self.assertEqual(len(errors), 1)
        self.assertIn("0 fences titled workload.sq", errors[0])

    def test_a_fence_inside_a_longer_fence_is_its_body(self):
        # a page that shows how to write a fence is not a program
        text = f"{FENCE}`markdown\n{FENCE}serq\n{MODEL}{FENCE}\n{FENCE}`\n"
        self.assertEqual(check(text), ([], 0, 0))

    def test_a_serq_that_runs_on_is_stopped_at_the_deadline(self):
        # a stand-in serq that never returns: the check fails at the page line, not at CI's job limit
        with tempfile.TemporaryDirectory() as d:
            hang = Path(d) / "serq"
            hang.write_text("#!/bin/sh\necho linking\nexec sleep 30\n")
            hang.chmod(0o755)
            with mock.patch.object(docs, "DEADLINE", 1), mock.patch.object(docs, "SERQ", str(hang)):
                errors, linked, _ = check(pair())
        self.assertEqual(linked, 1)
        self.assertEqual(len(errors), 1)
        self.assertTrue(errors[0].startswith("page.md:1:"), errors[0])
        self.assertIn("ran past 1 s and was stopped\nlinking", errors[0])


class Python(unittest.TestCase):
    def test_a_script_is_found_by_its_import_whatever_its_language(self):
        text = "".join(f"{FENCE}{lang}\nimport pyserq\n{FENCE}\n" for lang in ("python", "py", ".Python3", "text"))
        text += f"{FENCE}python\nimport json\n{FENCE}\n"
        with tempfile.TemporaryDirectory() as d:
            page = Path(d) / "page.md"
            page.write_text(text)
            self.assertEqual([line for line, _ in docs.pyserq_scripts(page)], [1, 4, 7, 10])

    def test_a_script_is_found_by_any_import_of_pyserq(self):
        for imports in ("from pyserq import compile", "import pyserq as sq", "import json, pyserq",
                        "import json as j, pyserq as sq", "from pyserq.sub import x", "    import pyserq"):
            with self.subTest(imports=imports):
                self.assertTrue(docs.PYSERQ.search(f"x = 1\n{imports}\n"))
        for other in ("import pyserqx", "from pyserq_helper import x", "# import pyserq",
                      "import json  # pyserq", 'print("import pyserq")'):
            with self.subTest(other=other):
                self.assertFalse(docs.PYSERQ.search(f"{other}\n"))

    def test_a_failing_script_names_its_page_line(self):
        e = docs.run_python("raise SystemExit('no')\n", "page.md:5")
        self.assertTrue(e.startswith("page.md:5:"), e)
        self.assertIsNone(docs.run_python("assert 1 + 1 == 2\n", "page.md:5"))

    def test_a_script_that_runs_on_is_stopped_at_the_deadline(self):
        with mock.patch.object(docs, "DEADLINE", 1):
            e = docs.run_python("print('started', flush=True)\nwhile True:\n    pass\n", "page.md:5")
        self.assertTrue(e.startswith("page.md:5:"), e)
        self.assertIn("ran past 1 s and was stopped\nstarted", e)

    # Only against a wheel of this tree: the repository's pyserq/ directory, or an
    # older pyserq installed here, would be found as "pyserq" and answer for another
    # language (one before #390 links a bare-number cost).
    @unittest.skipUnless(os.environ.get("PYSERQ_FROM_TREE"),
                         "PYSERQ_FROM_TREE is unset: the pyserq found here may not be this tree's")
    def test_a_bare_number_cost_fails_in_python(self):
        # #497 as it was found: a pyserq page's program that no longer links
        body = ("import pyserq\n"
                f"source = '''{MODEL.replace('COST', '1')}'''\n"
                f"pyserq.run(pyserq.compile(source=source, workload_source='''{WORKLOAD}''', horizon=10))\n")
        e = docs.run_python(body, "page.md:1")
        self.assertIn("cost type", e)


if __name__ == "__main__":
    unittest.main()
