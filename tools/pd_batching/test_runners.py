#!/usr/bin/env python3
"""Exercise experiment invocations without rewriting their recorded results."""

import csv
import math
import unittest

import sessions
import sweep


class ExecutionSettings(unittest.TestCase):
    def test_ps_uses_explicit_conditions_and_the_requested_seed(self):
        report = sweep.run(sweep.PS, {"mode": 0, "Lambda": 5}, {}, 2)
        self.assertEqual((report["horizon"], report["warmup"], report["seed"]), (300, 30, 2))
        self.assertGreater(report["observes"]["decode_time"]["count"], 0)

    def test_edited_step_model_uses_the_experiment_conditions(self):
        report = sweep.run(
            sweep.STEP, {"mode": 1, "Lambda": 0.1}, {"prompt_len": "2000"}, 2,
            [("let NP = 3;", "let NP = 2;")],
        )
        self.assertEqual((report["horizon"], report["warmup"], report["seed"]), (300, 30, 2))
        self.assertGreater(report["observes"]["decode_time"]["count"], 0)

    def test_sessions_uses_explicit_conditions_and_the_requested_seed(self):
        report = sessions.run("split", "open", 0.01, 2)
        self.assertEqual((report["horizon"], report["warmup"], report["seed"]), (2000, 200, 2))
        self.assertGreater(report["observes"]["P.prefill"]["count"], 0)


class Results(unittest.TestCase):
    """results.csv is what the committed programs print: two cheap rows,
    rerun on seed 1, agree with it. A change that moves a seeded number
    moves them, and the file is regenerated with it (#492)."""

    ROWS = [("ps", "colocated", 5, "-"), ("step", "colocated", 20, "fixed")]

    def test_two_rows_reproduce(self):
        with open(sweep.HERE / "results.csv") as f:
            recorded = {(r["experiment"], r["mode"], r["lambda"], r["prompt"], r["seed"]): r for r in csv.DictReader(f)}
        for experiment, program, mode, lam, prompt, sets, defs, edits in sweep.grid():
            name = "colocated" if mode == 0 else "split"
            if (experiment, name, lam, prompt) not in self.ROWS:
                continue
            where = f"{experiment} {name} lambda {lam} {prompt}"
            old = recorded.get((experiment, name, str(lam), prompt, "1"))
            if old is None:
                self.fail(f"{where}: no row in results.csv; regenerate with sweep.py")
            now = sweep.row(experiment, mode, lam, prompt, 1, sweep.run(program, sets, defs, 1, edits), edits)
            for k, v in now.items():
                if isinstance(v, float) and not math.isnan(v) and not math.isclose(v, float(old[k]), rel_tol=1e-9):
                    self.fail(f"{where}: {k} is {v}, results.csv has {old[k]}; regenerate with sweep.py")


if __name__ == "__main__":
    unittest.main()
