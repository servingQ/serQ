#!/usr/bin/env python3
"""The vLLM scheduler scenarios answered three ways must agree: the CPU
oracle on upstream vLLM (tools/oracle/*.out.json), the real A100
engine (tools/oracle/a100_engine.json), and serQ (tests/ and
lean/Serq/Oracle.lean check against *.out.json)."""
import json
import os
import sys

D = os.path.join(os.path.dirname(__file__), "..", "tools", "oracle")
gpu = json.load(open(os.path.join(D, "a100_engine.json")))["results"]
bad = 0
for name, g in sorted(gpu.items()):
    o = json.load(open(os.path.join(D, name + ".out.json")))
    for key in ["first", "done", "preemptions"]:
        if o[key] != g[key]:
            print(f"MISMATCH {name}.{key}: cpu oracle {o[key]} vs A100 engine {g[key]}")
            bad += 1
print(f"checked {len(gpu)} scenarios: CPU oracle (vLLM 0.30.1rc0+215) = A100 engine (vLLM 0.30.0)")
sys.exit(1 if bad else 0)
