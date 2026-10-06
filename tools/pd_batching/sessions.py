#!/usr/bin/env python3
"""Colocated vs split on a deployment that is not built for the question
(#208): examples/pd-disaggregation/llmd_nixl_pull.sq, the llm-d router in
front of vLLM pods with the NIXL connector, multi-turn sessions that call a
tool between turns and reuse their prefix. Four engines either way:

- colocated: 4 decode pods that prefill every prompt themselves (`thr`
  past any prompt, so the router's decider never asks for a remote
  prefill; the one prefill pod the family needs stays idle);
- colocated_cache: the same 4 pods, the router choosing one that has the
  session's prefix before the least loaded, as it chooses a prefill pod
  (the decode profile as written only weighs load);
- split: 3 prefill pods and 1 decode pod, every prompt prefilled remotely
  (`thr = 1`, the guide's always-disagg decider).

First an open workload (sessions arrive at `Lambda` per second), then a
closed one (`arrive closed(U)`: U users, each starting a new session when
one ends), where a faster answer brings the next turn sooner.

    cargo build --release
    tools/pd_batching/sessions.py      # about fifteen minutes; writes sessions.csv, sessions.md here

Simulation of the committed program with the edits below, not a
measurement. The pods' engines are the program's: vLLM's default mixed
batches, no exclusive prefill.
"""

import csv
import json
import math
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
SERQ = os.environ.get("SERQ", str(ROOT / "target/release/serq"))
LLMD = ROOT / "examples/pd-disaggregation/llmd_nixl_pull.sq"
SEEDS = [1, 2, 3, 4, 5]
T95 = {1: 12.706, 2: 4.303, 3: 3.182, 4: 2.776, 5: 2.571, 6: 2.447, 7: 2.365, 8: 2.306, 9: 2.262}

# (the program's text, what it becomes), and `--set`s
FOUR_DECODERS = [("let NP = 2;", "let NP = 1;"), ("let ND = 2;", "let ND = 4;")]
CACHE_ROUTER = (
    "    choose j in ND by (holders(D[j].kv) + queued(D[j].kv));",
    "    choose j in ND by (cachedin(D[j].kv) > 0 ? 0 : 1, holders(D[j].kv) + queued(D[j].kv));",
)
MODES = {
    "colocated": (FOUR_DECODERS, {"thr": 1e9}),
    "colocated_cache": ([*FOUR_DECODERS, CACHE_ROUTER], {"thr": 1e9}),
    "split": ([("let NP = 2;", "let NP = 3;"), ("let ND = 2;", "let ND = 1;")], {"thr": 1}),
}
# What the prefill pods compute of a prompt, which the program does not
# observe: the decode pods' own prefill is `local_tokens`.
P_TOKENS = ("      prefill (prompt - c) growing kv;      // chunked; one token is sampled and discarded",
            "      observe p_tokens = prompt - c;\n      prefill (prompt - c) growing kv;      // chunked; one token is sampled and discarded")
OPEN = [1, 2, 3]          # sessions per second
CLOSED = [20, 40, 60]     # users


def edits_of(mode, workload, x):
    edits = [*MODES[mode][0], P_TOKENS, ('use "../../lib/vllm.sq";', f'use "{ROOT / "lib/vllm.sq"}";')]
    if workload == "closed":
        edits.append(("arrive poisson(Lambda);", f"arrive closed({x});"))
    return edits


def run(mode, workload, x, seed):
    sets = MODES[mode][1] if workload == "closed" else {**MODES[mode][1], "Lambda": x}
    text = LLMD.read_text()
    for old, new in edits_of(mode, workload, x):
        # each edit is one line of the program, found once
        if text.count(old) != 1:
            sys.exit(f"{LLMD}: {old!r} occurs {text.count(old)} times, not once")
        text = text.replace(old, new)
    with tempfile.TemporaryDirectory() as d:
        program = Path(d) / LLMD.name
        program.write_text(text)
        instance = LLMD.parent / "instances" / LLMD.stem / "default.sq"
        cmd = [SERQ, "run", str(program), "--instance", str(instance), "--json", "--seed", str(seed)]
        for k, v in sets.items():
            cmd += ["--set", f"{k}={v}"]
        return json.loads(subprocess.run(cmd, check=True, capture_output=True, text=True).stdout)


def mean(xs):
    xs = [x for x in xs if x is not None and not math.isnan(x)]
    return sum(xs) / len(xs) if xs else math.nan


def num(x):
    return math.nan if x is None else x


def row(mode, workload, x, seed, rep):
    span = rep["end"] - rep["warmup"]
    obs = rep["observes"]
    dec = [s for s in rep["stages"] if s["name"] == "D" and s["iterations"] > 0]
    pods = [s for s in rep["stages"] if s["name"] in ("P", "D") and s["iterations"] > 0]
    split = mode == "split"
    # a pod's mean gap weighs by its decode tokens a second: the decodes
    # in its running iteration over the step that carries them
    weight = [num(s["mean_decodes"]) / num(s["mean_decode_step"]) for s in dec]
    gaps = [num(s["mean_itl"]) for s in dec]
    return {
        "mode": mode,
        "workload": workload,
        "x": x,
        "seed": seed,
        "turns_per_s": rep["turns"] / span,
        "live": rep["mean_live"],
        "ttft": obs["ttft"]["mean"],
        "response": obs["response"]["mean"],
        "decode": obs["response"]["mean"] - obs["ttft"]["mean"],
        "remote": obs["remote"]["mean"],
        # prompt tokens computed by whichever pod prefills, per prefill
        # (a turn, but for the few preempted and run again)
        "prefill_tokens": obs["p_tokens" if split else "local_tokens"]["mean"],
        "mean_itl": sum(w * g for w, g in zip(weight, gaps)) / sum(weight),
        "itl_p99": max([num(s["itl_p99"]) for s in dec], default=math.nan),
        "batch": mean([num(s["mean_decode_batch"]) for s in dec]),
        # the busy fraction of the engines that carry prefills: the decode
        # pods colocated, the prefill pods split
        "prefill_busy": mean([s["prefill_only"] + s["mixed"] for s in pods if (s["name"] == "D") != split]),
        "preemptions_per_s": sum(p["preemptions"] for p in rep["pools"]) / span,
        "stuck": sum(p["stuck"] for p in rep["pools"]),
        # the edits that make the deployment compared (not the observe and
        # the library's path, which every run has)
        "edits": json.dumps([e for e in edits_of(mode, workload, x) if e[0] != P_TOKENS[0] and not e[0].startswith("use ")]),
    }


def ci(xs):
    xs = [x for x in xs if not math.isnan(x)]
    n = len(xs)
    if n == 0:
        return math.nan, math.nan
    m = sum(xs) / n
    if n < 2:
        return m, math.nan
    sd = math.sqrt(sum((v - m) ** 2 for v in xs) / (n - 1))
    return m, T95[n - 1] * sd / math.sqrt(n)


def fmt(xs, scale=1.0, digits=3):
    m, h = ci([v * scale for v in xs])
    return f"{m:.{digits}f} ± {h:.{digits}f}"


def main():
    rows = []
    for workload, xs in [("open", OPEN), ("closed", CLOSED)]:
        for mode in MODES:
            for x in xs:
                for seed in SEEDS:
                    rows.append(row(mode, workload, x, seed, run(mode, workload, x, seed)))
                    print(f"{workload} {mode} {x} seed {seed}", file=sys.stderr)
    with open(HERE / "sessions.csv", "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0]))
        w.writeheader()
        w.writerows(rows)
    out = [
        "# Colocated vs split on llm-d's deployment: the sessions",
        "",
        "Generated by `tools/pd_batching/sessions.py` from `examples/pd-disaggregation/llmd_nixl_pull.sq` (4 decode pods prefilling locally, or 3 prefill pods and 1 decode pod); simulation, not a measurement.",
        f"Mean ± 95 % CI over seeds {SEEDS[0]}–{SEEDS[-1]} (Student t). Times in milliseconds. `x` is sessions per second (open) or users (closed).",
        "`decode` is response − TTFT; `prefill tok` is the prompt tokens a prefill computes (the rest is a prefix hit; a turn's, but for the few run again after a preemption); `ITL` is the decode pods' `mean_itl` weighed by their decode tokens, and the largest `itl_p99`; `prefill busy` is the time the engines that prefill run a prefill or a mixed batch. A mean ITL above the p99 is the gaps beyond the p99: the prefills a decode rides with.",
        "",
        "| workload | x | mode | turns/s | live | TTFT | decode | response | prefill tok | ITL | ITL p99 | batch | prefill busy | preempt/s | stuck |",
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    groups = {}
    for r in rows:
        groups.setdefault((r["workload"], r["x"], r["mode"]), []).append(r)
    for (workload, x, mode), g in groups.items():
        out.append(
            f"| {workload} | {x} | {mode} | {fmt([r['turns_per_s'] for r in g], 1, 2)} | {fmt([r['live'] for r in g], 1, 1)} | "
            f"{fmt([r['ttft'] for r in g], 1e3, 1)} | {fmt([r['decode'] for r in g], 1e3, 1)} | {fmt([r['response'] for r in g], 1e3, 1)} | "
            f"{fmt([r['prefill_tokens'] for r in g], 1, 0)} | {fmt([r['mean_itl'] for r in g], 1e3)} | {fmt([r['itl_p99'] for r in g], 1e3, 2)} | {fmt([r['batch'] for r in g], 1, 2)} | "
            f"{fmt([r['prefill_busy'] for r in g], 1, 2)} | {fmt([r['preemptions_per_s'] for r in g], 1, 2)} | {sum(r['stuck'] for r in g)} |"
        )
    (HERE / "sessions.md").write_text("\n".join(out) + "\n")


if __name__ == "__main__":
    main()
