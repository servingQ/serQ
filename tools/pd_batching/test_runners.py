#!/usr/bin/env python3
"""Exercise experiment invocations without rewriting their recorded results."""

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
        self.assertGreater(report["observes"]["p_tokens"]["count"], 0)


if __name__ == "__main__":
    unittest.main()
