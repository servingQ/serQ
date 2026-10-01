"""FIFO v7-v10 translation and explicit boundaries for newer IR.

Run from the repository root: python3 scripts/test_lean_oracle.py
"""
import copy
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import gen_lean_oracle as generator


class FragmentBoundaries(unittest.TestCase):
    def setUp(self):
        self.ir, _ = generator.load("mixed")

    def test_fifo_translates_the_same_across_supported_versions(self):
        reference = generator.Lean(self.ir)
        for version in generator.SUPPORTED_IR_VERSIONS:
            ir = copy.deepcopy(self.ir)
            ir["version"] = version
            with tempfile.TemporaryDirectory() as directory:
                Path(directory, "mixed.ir.json").write_text(json.dumps(ir))
                with patch.object(generator, "ODIR", directory):
                    _, lean = generator.load("mixed")
            self.assertEqual(reference.deployment(), lean.deployment())
            self.assertEqual(reference.block(ir["session"], 1), lean.block(ir["session"], 1))

    def test_unknown_versions_and_shared_execution_are_rejected(self):
        for update, message in [({"version": 99}, "IR version 99"),
                                ({"share": "MaxMin"}, "shared multi-stage")]:
            ir = {**copy.deepcopy(self.ir), **update}
            with tempfile.TemporaryDirectory() as directory:
                Path(directory, "mixed.ir.json").write_text(json.dumps(ir))
                with patch.object(generator, "ODIR", directory):
                    with self.assertRaisesRegex(generator.Fragment, message):
                        generator.load("mixed")

    def test_selection_keys_remain_outside_the_fragment(self):
        ir = copy.deepcopy(self.ir)
        ir["pools"][0]["queue"] = [{"Ctx": "Waited"}]
        with self.assertRaisesRegex(generator.Fragment, "FIFO queue"):
            generator.Lean(ir).deployment()

    def test_multistage_runs_are_not_silently_discarded(self):
        ir = copy.deepcopy(self.ir)
        for block in ir["blocks"]:
            for stmt in block:
                if "Run" in stmt:
                    stmt["Run"]["also"] = [{"base": 0, "count": 1, "index": None}]
                    with self.assertRaisesRegex(generator.Fragment, "also"):
                        generator.Lean(ir).block(ir["session"], 1)
                    return
        self.fail("the mixed oracle must contain a Run")

    def test_leases_are_not_silently_discarded(self):
        ir = copy.deepcopy(self.ir)
        for block in ir["blocks"]:
            for stmt in block:
                if "Hold" in stmt:
                    stmt["Hold"]["lease"] = [{"base": 0, "count": 1, "index": None}, {"Num": 1}]
                    with self.assertRaisesRegex(generator.Fragment, "lease"):
                        generator.Lean(ir).block(ir["session"], 1)
                    return
        self.fail("the mixed oracle must contain a Hold")


if __name__ == "__main__":
    unittest.main()
