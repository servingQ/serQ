#!/usr/bin/env python3
"""The study's edits, measures and recorded rows (tools/metastability/study.py)."""

import csv
import math
import subprocess
import tempfile
import unittest
from pathlib import Path

import study

W = study.WINDOW


def series(hits, q=None, adm=100):
    """A window series with the given full-hit fractions and backlog."""
    n = len(hits)
    q = q or [0.0] * n
    return {
        "adm": [adm if h is not None else 0 for h in hits],
        "full": [round(adm * h) if h is not None else 0 for h in hits],
        "q": list(q),
    }


class Edits(unittest.TestCase):
    def test_every_arm_edits_its_line_once_and_links_under_both_workloads(self):
        study.check_workloads()
        for arm, (_title, edits, _sets) in study.ARMS.items():
            for workload in [study.CLOSED, study.OPEN]:
                text = study.program_text(workload, edits)  # asserts each edit occurs once
                for _old, new in edits:
                    self.assertIn(new, text, arm)
                with tempfile.TemporaryDirectory() as tmp:
                    program = Path(tmp) / study.PROGRAM.name
                    program.write_text(text)
                    out = subprocess.run(
                        [study.SERQ, "check", str(program), "--workload", str(workload)], capture_output=True, text=True
                    )
                    self.assertEqual(out.returncode, 0, f"{arm} under {workload.name}: {out.stderr}")

    def test_an_edit_that_does_not_occur_once_is_refused(self):
        with self.assertRaises(AssertionError):
            study.program_text(study.CLOSED, [("evict mru;", "evict lru;")])


class Measures(unittest.TestCase):
    def test_a_dip_shorter_than_four_windows_is_not_a_collapse(self):
        s = series([0.8] * 10 + [0.0] * 3 + [0.8] * 10)
        self.assertIsNone(study.collapse_time(s, 23))

    def test_a_collapse_starts_at_its_first_window(self):
        s = series([0.8] * 10 + [0.05] * 4 + [0.8] * 10)
        self.assertEqual(study.collapse_time(s, 24), 10 * W)

    def test_a_window_without_admissions_breaks_a_collapse(self):
        s = series([0.8] * 10 + [0.0, 0.0, None, 0.0, 0.0] + [0.8] * 10)
        self.assertIsNone(study.collapse_time(s, 25))

    def test_collapsed_at_end_reads_the_last_500_s(self):
        n = 100
        self.assertTrue(study.collapsed_at_end(series([0.8] * (n - 20) + [0.0] * 20), n))
        self.assertFalse(study.collapsed_at_end(series([0.8] * (n - 20) + [0.0] * 19 + [0.5]), n))

    def test_recovery_counts_from_the_burst_end_to_four_windows_at_the_pre_burst_level(self):
        n = 80
        pre = int(study.BURST_START // W)  # windows 0 .. 39 at 0.8
        hits = [0.8] * pre + [0.0] * 10 + [0.75] * (n - pre - 10)
        s = series(hits)
        end = study.BURST_START + 5 * W  # the burst ends 5 windows before the hits come back
        self.assertEqual(study.recovery_time(s, n, end), 5 * W)

    def test_recovery_waits_for_the_backlog(self):
        n = 80
        pre = int(study.BURST_START // W)
        hits = [0.8] * pre + [0.8] * (n - pre)
        q = [1.0] * pre + [50.0] * 8 + [2.5] * (n - pre - 8)
        s = series(hits, q)
        self.assertEqual(study.recovery_time(s, n, study.BURST_START), 8 * W)

    def test_never_recovering_is_inf_and_a_collapse_before_the_burst_is_none(self):
        n = 80
        pre = int(study.BURST_START // W)
        self.assertTrue(math.isinf(study.recovery_time(series([0.8] * pre + [0.0] * (n - pre)), n, study.BURST_START)))
        before = [0.8] * 25 + [0.0] * 5 + [0.8] * (pre - 30) + [0.8] * (n - pre)
        self.assertIsNone(study.recovery_time(series(before), n, study.BURST_START))

    def test_growing_needs_more_than_noise(self):
        n = 160
        self.assertFalse(study.growing(series([0.9] * n, [3.0 + (k % 2) for k in range(n)]), n))
        self.assertTrue(study.growing(series([0.0] * n, [0.5 * k for k in range(n)]), n))

    def test_a_gauge_is_averaged_over_the_time_it_holds_each_value(self):
        # 0 until 10 s, 4 until 30 s, then 1: windows [0, 25) and [25, 50)
        got = study.average([(0.0, 0.0), (10.0, 4.0), (30.0, 1.0)], 2, 50.0)
        for x, y in zip(got, [4 * 15 / 25, (4 * 5 + 1 * 20) / 25]):
            self.assertAlmostEqual(x, y)

    def test_the_poisson_tail_counts_past_the_contexts(self):
        # P(X > 1.5) = P(X >= 2) = 1 - e^-m (1 + m); a whole c is not exceeded at c
        m = 2.0
        self.assertAlmostEqual(study.poisson_above(m, 1.5), 1 - math.exp(-m) * (1 + m))
        self.assertAlmostEqual(study.poisson_above(m, 2.0), 1 - math.exp(-m) * (1 + m + m * m / 2))


class Execution(unittest.TestCase):
    def test_a_run_uses_the_conditions_it_names(self):
        c = study.cell("E4", study.OPEN, "lru", 0.3, "default", 0, study.OPEN_PEAK, [200])
        rep, s = study.run(c["workload"], c["edits"], c["sets"], 2, 200)
        self.assertEqual((rep["horizon"], rep["warmup"], rep["seed"]), (200, 0, 2))
        self.assertEqual(len(s["adm"]), 8)
        self.assertGreater(sum(s["adm"]), 0)


class Results(unittest.TestCase):
    """results.csv is what the committed programs print: two cheap rows,
    rerun on seed 1, agree with it. The second is read in the study from
    a run of 8000 s and rerun here for 2000 s, which also holds a short
    run to be the start of a long one. A change that moves a seeded number
    moves them, and the files are regenerated with it (#492)."""

    ROWS = [("E1", "lru", 4.0, "default", 60, 2000), ("E4", "lru", 0.4, "default", 0, 2000)]

    def test_two_rows_reproduce(self):
        with open(study.HERE / "results.csv") as f:
            recorded = {
                (r["experiment"], r["arm"], float(r["load"]), r["start"], int(r["burst"]), int(r["horizon"]), int(r["seed"])): r
                for r in csv.DictReader(f)
            }
        for c in study.cells():
            for key in self.ROWS:
                if (c["experiment"], c["arm"], c["load"], c["start"], c["burst"]) != key[:5]:
                    continue
                horizon = key[5]
                where = " ".join(map(str, key))
                old = recorded.get((*key, 1))
                if old is None:
                    self.fail(f"{where}: no row in results.csv; regenerate with study.py")
                _rep, s = study.run(c["workload"], c["edits"], c["sets"], 1, horizon)
                (now,) = study.rows(c, 1, s, [horizon])
                for k, v in now.items():
                    text = study.cell_text(v)
                    if isinstance(v, float) and not math.isnan(v):
                        same = math.isclose(v, float(old[k]), rel_tol=1e-9)
                    else:
                        same = str(text) == old[k]
                    if not same:
                        self.fail(f"{where}: {k} is {text}, results.csv has {old[k]}; regenerate with study.py")


if __name__ == "__main__":
    unittest.main()
