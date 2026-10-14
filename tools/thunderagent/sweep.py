#!/usr/bin/env python3
"""ThunderAgent's TR mode against its default mode
(examples/multi-turn/thunderagent.sq, docs/design/thunderagent.md): run
both over the agent counts and print, per count, the steps per minute of
each, their ratio, the token hit rate of each, and how often TR mode
lets a paused program in again.

    cargo build --release
    tools/thunderagent/sweep.py                       # the example as committed
    tools/thunderagent/sweep.py --preset paper        # the paper's scale
    tools/thunderagent/sweep.py --preset paper g=1.1e-9 S=0

`k=v` sets an argument of the program after the preset's. Every number is
a simulation of the program as committed, not a measurement: a steps/min
is turns per minute after warm-up, the paper's unit (one reasoning and one
acting period), and the hit rate is reused prompt tokens over prompt
tokens, vLLM's prefix_cache_hits over prefix_cache_queries.
"""

import json
import os
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SERQ = os.environ.get("SERQ", str(ROOT / "target/release/serq"))
PROGRAM = ROOT / "examples/multi-turn/thunderagent.sq"
sys.path.insert(0, str(ROOT / "scripts"))
import pairs  # noqa: E402  (each program's workload: examples/pairs.json)

WORKLOAD = pairs.workload(PROGRAM)
# long enough that the contexts' distribution has settled (a shorter run
# reads it before the longest agents have grown)
HORIZON, WARMUP = 25000, 5000

# The paper's scale (docs/design/thunderagent.md, *Why the numbers differ*):
# each replica a GLM-4.6-class model (355B, 32B active) in FP8 weights on
# one 8xH100 node with a BF16 KV of 377 KB a token (92 layers, 8 KV heads of
# 128): about 500 000 tokens of KV; 64 GFLOP a token at 4 PFLOP/s, about
# half the node's BF16 peak; 15 ms to read the weights an iteration, 14 ns a
# context token's KV; a prefill's attention 4.5 MFLOP a token pair, 1.1 ns
# at that rate and 3.3 ns at a third of it. Every program starts with the
# same 3 968-token system prompt. Both replicas together hold about 50 of
# the agents' contexts. None of it is fitted: the paper states none of it.
PRESETS = {
    "example": {"ns": [4, 8, 16, 32, 48], "sets": {}},
    "paper": {
        "ns": [24, 48, 96, 144, 192],
        "sets": {"blocks": 31250, "a": 1.6e-5, "omega": 0.015, "beta": 1.4e-8, "g": 3.3e-9,
                 "S": 3968},
    },
}


def run(n, tr, seed, sets):
    args = [SERQ, "run", str(PROGRAM), "--workload", str(WORKLOAD), "--horizon", str(HORIZON), "--warmup", str(WARMUP),
            "--seed", str(seed), "--json", "--tr", str(tr), "--N", str(n)]
    for k, v in sets.items():
        args += [f"--{k}", str(v)]
    r = json.loads(subprocess.run(args, capture_output=True, text=True, check=True).stdout)
    o = r["observes"]
    total = lambda k: o[k]["mean"] * o[k]["count"] if o.get(k) and o[k]["count"] else 0.0
    reused, fresh = total("E.reused"), total("E.prefill")
    resumes = o["resume_wait"]["count"] if o.get("resume_wait") else 0
    return {
        "spm": r["turns"] / (r["end"] - r["warmup"]) * 60,
        "hit": reused / (reused + fresh) if reused + fresh else 0.0,
        "resumed": resumes / r["turns"] if r["turns"] else 0.0,
    }


def main(argv):
    preset, seeds, extra = "example", [1, 2, 3, 4], {}
    it = iter(argv)
    for a in it:
        if a == "--preset":
            preset = next(it)
        elif a == "--seeds":
            seeds = [int(x) for x in next(it).split(",")]
        else:
            k, v = a.split("=", 1)
            extra[k] = v
    ns, sets = PRESETS[preset]["ns"], {**PRESETS[preset]["sets"], **extra}
    jobs = [(n, tr, s) for n in ns for tr in (1, 0) for s in seeds]
    with ThreadPoolExecutor(os.cpu_count() or 4) as ex:
        res = dict(zip(jobs, ex.map(lambda j: run(*j, sets), jobs)))
    mean = lambda rows, k: sum(r[k] for r in rows) / len(rows)
    print(f"preset {preset}, seeds {seeds}, {' '.join(f'{k}={v}' for k, v in sets.items())}")
    print("| N | TR steps/min | default steps/min | ratio | TR hit | default hit | TR resumed/turn |")
    print("|---|---|---|---|---|---|---|")
    for n in ns:
        t = [res[(n, 1, s)] for s in seeds]
        d = [res[(n, 0, s)] for s in seeds]
        print(f"| {n} | {mean(t, 'spm'):.1f} | {mean(d, 'spm'):.1f} | "
              f"{mean(t, 'spm') / mean(d, 'spm'):.2f} | {mean(t, 'hit'):.2f} | "
              f"{mean(d, 'hit'):.2f} | {mean(t, 'resumed'):.3f} |")


if __name__ == "__main__":
    main(sys.argv[1:])
