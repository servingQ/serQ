"""scripts/check_docs_examples.py refuses what it says it refuses.

Run from the repository root: python3 scripts/test_check_docs_examples.py
(needs target/release/serq; the pyserq case needs pyserq installed and is
skipped, by name, without it)
"""
import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

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
        self.assertTrue(errors[0].startswith("page.md:18:"), errors[0])
        self.assertIn("no horizon", errors[0])

    def test_a_continued_command_runs_whole(self):
        self.assertEqual(check(pair(run="serq run model.sq \\\n  --workload workload.sq --horizon 10")),
                         ([], 1, 1))

    def test_a_main_with_no_title_and_no_marker_is_refused(self):
        errors, _, _ = check(f"{FENCE}serq\n{MODEL}{FENCE}\n")
        self.assertEqual(len(errors), 1)
        self.assertIn("{ .excerpt }", errors[0])

    def test_the_brace_spelling_is_read_too(self):
        errors, _, _ = check(f"{FENCE}{{ .serq }}\n{MODEL}{FENCE}\n")
        self.assertEqual(len(errors), 1)
        self.assertIn("{ .excerpt }", errors[0])
        self.assertEqual(check(f"{FENCE}{{ .serq .excerpt }}\nfn main() {{ … }}\n{FENCE}\n"), ([], 0, 0))

    def test_an_excerpt_is_not_linked(self):
        self.assertEqual(check(f"{FENCE}serq {{ .excerpt }}\nfn main() {{ … }}\n{FENCE}\n"), ([], 0, 0))

    def test_a_main_named_in_a_comment_is_a_fragment(self):
        self.assertEqual(check(f"{FENCE}serq\n// in fn main()\nserver {{ … }}\n{FENCE}\n"), ([], 0, 0))

    def test_a_marker_on_a_fragment_is_refused(self):
        errors, _, _ = check(f"{FENCE}serq {{ .excerpt }}\npool kv {{ cap 1; }}\n{FENCE}\n")
        self.assertEqual(len(errors), 1)
        self.assertIn("drop the marker", errors[0])

    def test_a_marker_on_a_titled_program_is_refused(self):
        errors, _, _ = check(pair().replace('title="model.sq"', 'title="model.sq" { .excerpt }'))
        self.assertEqual(len(errors), 1)
        self.assertIn("drop the marker", errors[0])

    def test_a_model_needs_its_workload(self):
        errors, _, _ = check(f'{FENCE}serq title="model.sq"\n{MODEL}{FENCE}\n')
        self.assertEqual(len(errors), 1)
        self.assertIn("0 fences titled workload.sq", errors[0])

    def test_a_fence_inside_a_longer_fence_is_its_body(self):
        # a page that shows how to write a fence is not a program
        text = f"{FENCE}`markdown\n{FENCE}serq\n{MODEL}{FENCE}\n{FENCE}`\n"
        self.assertEqual(check(text), ([], 0, 0))


class Python(unittest.TestCase):
    def test_a_failing_script_names_its_page_line(self):
        e = docs.run_python("raise SystemExit('no')\n", "page.md:5")
        self.assertTrue(e.startswith("page.md:5:"), e)
        self.assertIsNone(docs.run_python("assert 1 + 1 == 2\n", "page.md:5"))

    @unittest.skipUnless(importlib.util.find_spec("pyserq"), "pyserq is not installed")
    def test_a_bare_number_cost_fails_in_python(self):
        # #497 as it was found: a pyserq page's program that no longer links
        body = ("import pyserq\n"
                f"source = '''{MODEL.replace('COST', '1')}'''\n"
                f"pyserq.run(pyserq.compile(source=source, workload_source='''{WORKLOAD}''', horizon=10))\n")
        e = docs.run_python(body, "page.md:1")
        self.assertIn("cost type", e)


if __name__ == "__main__":
    unittest.main()
