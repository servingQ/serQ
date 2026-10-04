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



class IterationCost(unittest.TestCase):
    def test_the_step_clock_is_the_constant_one(self):
        ir, _ = generator.load("mixed")
        self.assertEqual(generator.cost_fn(ir["stages"][0]["kind"]["Step"]["cost"]), "fun _ => 1")

    def test_integer_affine_costs_translate(self):
        e = {"Binary": ["Add", {"Num": 4000.0},
                        {"Binary": ["Add", {"Binary": ["Mul", {"Num": 52.0}, {"Ctx": "Npre"}]},
                                    {"Binary": ["Mul", {"Num": 41.0}, {"Ctx": "Ndec"}]}]}]}
        self.assertEqual(generator.cost_fn(e), "fun st => 4000 + 52 * st.prefilled + 41 * st.decoders")

    # The expected values follow from the interpreter's arithmetic on
    # expressions (`binop` in src/link.rs: + - * on reals), which `affine`
    # normalises: (5 - Ntok) + Ntok is 5, 2 * (3 * Npre) is 6 Npre,
    # 2 * 3 is 6.
    def test_affine_normalises_as_the_interpreter_computes(self):
        num = lambda v: {"Num": float(v)}
        ctx = lambda v: {"Ctx": v}
        bi = lambda op, a, b: {"Binary": [op, a, b]}
        self.assertEqual(generator.cost_fn(bi("Add", bi("Sub", num(5), ctx("Ntok")), ctx("Ntok"))), "fun st => 5")
        self.assertEqual(generator.cost_fn(bi("Add", num(1), bi("Mul", num(2), bi("Mul", num(3), ctx("Npre"))))),
                         "fun st => 1 + 6 * st.prefilled")
        self.assertEqual(generator.cost_fn(bi("Mul", num(2), num(3))), "fun st => 6")

    def test_negative_or_zero_constant_costs_are_outside(self):
        num = lambda v: {"Num": float(v)}
        bi = lambda op, a, b: {"Binary": [op, a, b]}
        with self.assertRaises(generator.Fragment):
            generator.cost_fn(bi("Sub", num(4000), bi("Mul", num(2), {"Ctx": "Ndec"})))
        with self.assertRaises(generator.Fragment):  # an iteration of length 0 in the interpreter
            generator.cost_fn(bi("Mul", num(52), {"Ctx": "Npre"}))
        with self.assertRaises(generator.Fragment):
            generator.cost_fn(num(0))

    def test_fractional_or_unknown_costs_are_outside(self):
        with self.assertRaises(generator.Fragment):
            generator.cost_fn({"Binary": ["Mul", {"Num": 0.004}, {"Ctx": "Ndec"}]})
        with self.assertRaises(generator.Fragment):
            generator.cost_fn({"Binary": ["Add", {"Num": 1.0}, {"Binary": ["Mul", {"Num": 1.0}, {"Ctx": "Nres"}]}]})

    def test_an_odd_attention_coefficient_is_outside(self):
        # attention is a multiple of 1/2 (n^2/2 for a chunk of n at 0), so an
        # odd coefficient would leave the clock's natural numbers
        for c in (1.0, 3.0):
            with self.assertRaises(generator.Fragment):
                generator.cost_fn({"Binary": ["Add", {"Num": 1.0}, {"Binary": ["Mul", {"Num": c}, {"Ctx": "Attn"}]}]})

    def test_the_deployment_names_the_engine_memory(self):
        ir, lean = generator.load("mixed")
        self.assertIn(f", some {ir['stages'][0]['kind']['Step']['memory']}, fun _ => 1, none, ", lean.deployment())

    def test_the_chunk_is_a_constant_or_the_programs_expression(self):
        q = {"Call": ["Queued", [{"Pool": {"base": 0, "count": 1, "index": None}}]]}
        rule = {"Cond": [{"Binary": ["Gt", {"Binary": ["Add", {"Ctx": "Nres"}, q]}, {"Num": 1.0}]},
                         {"Num": 24.0}, {"Num": 0.0}]}
        self.assertEqual(generator.chunk_rule({"Num": 24.0}), (24.0, None))
        self.assertEqual(
            generator.chunk_rule(rule),
            (0, "some fun c => if ((c.residents + (c.queued 0)) > 1) then 24 else 0"))
        # not only vLLM's shape: the operands swapped, another threshold
        swapped = {"Cond": [{"Binary": ["Gt", {"Binary": ["Add", q, {"Ctx": "Nres"}]}, {"Num": 2.0}]},
                            {"Num": 24.0}, {"Num": 0.0}]}
        self.assertEqual(generator.chunk_rule(swapped)[1],
                         "some fun c => if (((c.queued 0) + c.residents) > 2) then 24 else 0")
        # what an iteration's start does not supply is outside the fragment
        with self.assertRaises(generator.Fragment):
            generator.chunk_rule({"Ctx": "Ntok"})
        # and so is a difference, which ℕ truncates where the interpreter does not
        sub = {"Cond": [{"Binary": ["Sub", {"Ctx": "Nres"}, {"Num": 1.0}]}, {"Num": 24.0}, {"Num": 0.0}]}
        with self.assertRaises(generator.Fragment):
            generator.chunk_rule(sub)
        # a rule whose outcomes are one constant is that constant
        zero = {"Cond": [rule["Cond"][0], {"Num": 0.0}, {"Num": 0.0}]}
        self.assertEqual(generator.chunk_rule(zero), (0.0, None))

    def test_a_constant_folds_to_the_interpreters_value(self):
        # an intermediate difference stays negative: 20 + (8 - 16) is 12
        e = {"Binary": ["Add", {"Num": 20.0}, {"Binary": ["Sub", {"Num": 8.0}, {"Num": 16.0}]}]}
        self.assertEqual(generator.fold(e), 12.0)
        self.assertEqual(generator.chunk_rule(e), (12.0, None))
        # a product with a zero constant is 0 whatever the other factor
        self.assertEqual(generator.fold({"Binary": ["Mul", {"Num": 0.0}, {"Attr": 3}]}), 0.0)

    def test_a_session_expression_reads_logic_and_ceil(self):
        x = generator.Expr(lambda e: f"(x.attr {e['Attr']})")
        a, b = {"Attr": 1}, {"Attr": 2}
        self.assertEqual(x.top({"Binary": ["And", {"Binary": ["Lt", a, b]}, {"Unary": ["Not", a]}]}),
                         "if (((x.attr 1) < (x.attr 2)) ∧ (¬ ((x.attr 1) ≠ 0))) then 1 else 0")
        ceil = {"Call": ["Ceil", [{"Expr": {"Binary": ["Div", a, {"Num": 16.0}]}}]]}
        self.assertEqual(x.top(ceil), "((x.attr 1) + 15) / 16")
        with self.assertRaises(generator.Fragment):
            x.top({"Call": ["Ceil", [{"Expr": {"Binary": ["Div", a, {"Num": 0.0}]}}]]})

    def test_attention_is_read_doubled_and_kv_decode_directly(self):
        e = {"Binary": ["Add", {"Num": 1.0},
                        {"Binary": ["Add", {"Binary": ["Mul", {"Num": 8.0}, {"Ctx": "Attn"}]},
                                    {"Binary": ["Mul", {"Num": 138.0}, {"Ctx": "Kvb"}]}]}]}
        self.assertEqual(generator.cost_fn(e), "fun st => 1 + 4 * st.attention2 + 138 * st.kvDecode")

if __name__ == "__main__":
    unittest.main()
