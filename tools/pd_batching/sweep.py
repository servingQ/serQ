#!/usr/bin/env python3
"""Colocated vs split engines on the same requests (#208): run
examples/pd-disaggregation/pd_ps.sq and pd_batching.sq over loads, prompt
laws, the variations of the baseline and seeds, and write what they report.

    cargo build --release
    tools/pd_batching/sweep.py            # writes raw.jsonl, results.csv, summary.md here

Every number is a simulation of the programs as committed, not a
measurement. A run is `stable` when the sessions it ended after warm-up
are within four Poisson deviations of the arrivals the rate brings in that
span; an unstable run is kept in raw.jsonl and results.csv and left out of
summary.md. The test catches a run that falls behind, not one that is
merely slow to reach steady state near saturation.
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
PS = ROOT / "examples/pd-disaggregation/pd_ps.sq"
STEP = ROOT / "examples/pd-disaggregation/pd_batching.sq"
SEEDS = [1, 2, 3, 4, 5]
# Student t, two-sided 95 %, by degrees of freedom
T95 = {1: 12.706, 2: 4.303, 3: 3.182, 4: 2.776, 5: 2.571, 6: 2.447, 7: 2.365, 8: 2.306, 9: 2.262}

# The step cases: three prompt laws of mean about 2000 tokens (the prefill
# work an engine sees has the same mean and a growing variance), at the
# program's 0.2 ms decode step and at a 10 ms one (`omega`), near the
# issue's example, where a colocated engine's decodes no longer drain
# between its prefills.
CASES = {
    "fixed": ("2000", {}, [20, 40, 60, 70]),
    "uniform": ("floor(~uniform(1000, 3000))", {}, [20, 40, 60, 70]),
    "h2cv9": ("max(1, floor(~h2(2000, 9)))", {}, [20, 40, 60, 70]),
    "fixed_w10ms": ("2000", {"omega": 0.01}, [40, 60, 70]),
}

EXCLUSIVE = "    serve exclusive prefill;\n"

# The variations of the baseline, one at a time, on prompts of 2000
# tokens: (title, the modes, the loads, `--set`s, edits of the program's
# text). An edit is a line the program states and the variation states
# otherwise: a policy (`serve exclusive prefill`) or a family's size
# (`NP`), which `--set` refuses.
VARIATIONS = {
    "mixed": ("mixed batches, whole prompts (`serve exclusive prefill` deleted)", [0, 1], [40, 60, 70], {}, [(EXCLUSIVE, "")]),
    "mixed_c512": ("mixed batches, prompts in chunks of 512 (`chunk_cap = 512`)", [0, 1], [40, 60, 70], {"chunk_cap": 512}, [(EXCLUSIVE, "")]),
    "mixed_w10ms": ("mixed batches, whole prompts, a 10 ms decode step (`omega = 0.01`)", [0, 1], [60], {"omega": 0.01}, [(EXCLUSIVE, "")]),
    "mixed_c512_w10ms": ("mixed batches in chunks of 512, a 10 ms decode step", [0, 1], [60], {"omega": 0.01, "chunk_cap": 512}, [(EXCLUSIVE, "")]),
    "bw_2e5": ("a read over NICs of 2e5 tokens/s (10 ms a prompt) after 2 ms (`Bw`, `x0`)", [1], [60, 70], {"Bw": 2e5, "x0": 0.002}, []),
    "bw_1e5": ("NICs of 1e5 tokens/s (20 ms a prompt): the decode engine's NIC takes every read", [1], [40, 45], {"Bw": 1e5, "x0": 0.002}, []),
    "kv_d_16k": ("a decode engine of 16384 KV tokens (`blocksD = 1024`)", [1], [60, 70], {"blocksD": 1024}, []),
    "kv_d_8k": ("a decode engine of 8192 KV tokens (`blocksD = 512`)", [1], [40, 45, 50], {"blocksD": 512}, []),
    "kv_e_8k": ("colocated engines of 8192 KV tokens each (`blocksE = 512`)", [0], [60, 65, 70], {"blocksE": 512}, []),
    "2p2d": ("2 prefill + 2 decode engines (`NP = 2`)", [0, 1], [30, 40], {}, [("let NP = 3;", "let NP = 2;")]),
    "2p2d_g15": ("2P/2D, prefill engines 1.5 times as fast (`gP = 1.5`)", [1], [40, 60], {"gP": 1.5}, [("let NP = 3;", "let NP = 2;")]),
}


def run(program, sets, defs, seed, edits=()):
    with tempfile.TemporaryDirectory() as dump:
        if edits:
            text = program.read_text()
            for old, new in edits:
                assert old in text, old
                text = text.replace(old, new)
            program = Path(dump) / program.name
            program.write_text(text)
        cmd = [SERQ, "run", str(program), "--horizon", "300", "--warmup", "30", "--json", "--seed", str(seed), "--dump", dump]
        for k, v in sets.items():
            cmd += ["--set", f"{k}={v}"]
        for k, v in defs.items():
            cmd += ["--def", f"{k}={v}"]
        out = subprocess.run(cmd, check=True, capture_output=True, text=True).stdout
        rep = json.loads(out)
        dec = column(Path(dump) / "decode_time.csv")
        out_tok = column(Path(dump) / "output_tokens.csv")
    # token-weighted TPOT: the decode time of every request over its
    # inter-token gaps, summed before dividing
    gaps = sum(out_tok[s] - 1 for s in dec)
    rep["tpot_token_weighted"] = sum(dec.values()) / gaps if gaps else math.nan
    rep["output_tokens_total"] = sum(out_tok.values())
    return rep


def column(path):
    with open(path) as f:
        return {(r["session"], r["turn"]): float(r["value"]) for r in csv.DictReader(f)}


def stages(rep, name):
    return [s for s in rep["stages"] if s["name"] == name]


def pool(rep, name):
    return [p for p in rep["pools"] if p["name"] == name]


def mean(xs):
    xs = [x for x in xs if x is not None and not math.isnan(x)]
    return sum(xs) / len(xs) if xs else math.nan


def row(experiment, mode, lam, prompt, seed, rep, edits=()):
    span = rep["end"] - rep["warmup"]
    obs = rep["observes"]
    r = {
        "experiment": experiment,
        "mode": "colocated" if mode == 0 else "split",
        "lambda": lam,
        "prompt": prompt,
        "seed": seed,
        # the program's lines a variation states otherwise
        "edits": json.dumps(edits) if edits else "",
        # ended after warm-up within four Poisson deviations of what the
        # rate brings in that span: a run that falls behind its arrivals
        # fails it; one that diverges slowly may not (summary.md says so)
        "stable": rep["ended"] >= lam * span - 4 * math.sqrt(lam * span),
        "requests_per_s": rep["ended"] / span,
        "output_tokens_per_s": rep["output_tokens_total"] / span,
        "decode_time": obs["decode_time"]["mean"],
        "tpot": obs["tpot"]["mean"],
        "tpot_token_weighted": rep["tpot_token_weighted"],
        "tpot_p99": obs["tpot"]["p99"],
    }
    if experiment == "ps":
        st = stages(rep, "colo" if mode == 0 else "dec")
        r["decoding_per_station"] = mean([s["mean_number"] for s in st])
        return r
    r["ttft"] = obs["ttft"]["mean"]
    r["response"] = obs["response"]["mean"]
    # the engines that decode: every colocated one, or the decode ones
    dec = stages(rep, "E" if mode == 0 else "D")
    r["decoding_per_engine"] = mean([s["mean_decodes"] for s in dec])
    r["batch_per_step"] = mean([s["mean_decode_batch"] for s in dec])
    r["decode_step"] = mean([s["mean_decode_step"] for s in dec])
    r["residents_per_engine"] = mean([p["mean_holders"] for p in pool(rep, "E.reqs" if mode == 0 else "D.reqs")])
    r["mean_itl"] = mean([s["mean_itl"] for s in dec])
    r["itl_p99"] = max([s["itl_p99"] for s in dec], default=math.nan)
    r["preemptions_per_s"] = sum(p["preemptions"] for p in pool(rep, "E.kv" if mode == 0 else "D.kv")) / span
    # the prefill engines' blocks, leased until the decoder reads them
    r["p_kv_used"] = sum(p["mean_used"] for p in pool(rep, "P.kv"))
    for k in ["admit_wait", "transfer", "lease"]:
        r[k] = obs[k]["mean"] if obs.get(k, {}).get("count") else math.nan
    for role in ["E", "P", "D"]:
        st = stages(rep, role)
        if st and st[0]["iterations"] > 0:
            for k in ["prefill_only", "decode_only", "mixed"]:
                r[f"{role}_{k}"] = mean([s[k] for s in st])
            r[f"{role}_idle"] = max(0.0, 1 - r[f"{role}_prefill_only"] - r[f"{role}_decode_only"] - r[f"{role}_mixed"])
    return r


def grid():
    for mode in [0, 1]:
        for lam in [5, 10, 15, 20]:
            yield "ps", PS, mode, lam, "-", {"mode": mode, "Lambda": lam}, {}, []
    for case, (law, sets, lams) in CASES.items():
        for mode in [0, 1]:
            for lam in lams:
                yield "step", STEP, mode, lam, case, {"mode": mode, "Lambda": lam, **sets}, {"prompt_len": law}, []
    for case, (_, modes, lams, sets, edits) in VARIATIONS.items():
        for mode in modes:
            for lam in lams:
                yield "variation", STEP, mode, lam, case, {"mode": mode, "Lambda": lam, **sets}, {"prompt_len": "2000"}, edits


def main():
    rows = []
    with open(HERE / "raw.jsonl", "w") as raw:
        for experiment, program, mode, lam, prompt, sets, defs, edits in grid():
            for seed in SEEDS:
                rep = run(program, sets, defs, seed, edits)
                r = row(experiment, mode, lam, prompt, seed, rep, edits)
                rows.append(r)
                params = {"program": str(program.relative_to(ROOT)), "edits": edits, "set": sets, "def": defs, "seed": seed}
                raw.write(json.dumps({"params": params, "report": rep}) + "\n")
                print(f"{experiment} {r['mode']:9} lambda {lam:3} {prompt:7} seed {seed}", file=sys.stderr)
    keys = []
    for r in rows:
        keys += [k for k in r if k not in keys]
    with open(HERE / "results.csv", "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=keys)
        w.writeheader()
        w.writerows(rows)
    (HERE / "summary.md").write_text(summary(rows))


def ci(xs):
    xs = [x for x in xs if not math.isnan(x)]
    n = len(xs)
    if n == 0:
        return math.nan, math.nan
    m = sum(xs) / n
    if n < 2:
        return m, math.nan
    sd = math.sqrt(sum((x - m) ** 2 for x in xs) / (n - 1))
    return m, T95[n - 1] * sd / math.sqrt(n)


def fmt(xs, scale=1.0, digits=3):
    m, h = ci([x * scale for x in xs])
    return f"{m:.{digits}f} ± {h:.{digits}f}"


def summary(rows):
    out = [
        "# Colocated vs split: the sweep",
        "",
        "Generated by `tools/pd_batching/sweep.py`; simulation of the committed programs, not a measurement.",
        f"Mean ± 95 % CI over seeds {SEEDS[0]}–{SEEDS[-1]} (Student t); runs that fell behind their arrivals are left out and counted. A point near saturation can pass and still be far from steady state in 300 s: see README.md.",
        "Times in milliseconds.",
        "",
        "## Processor sharing (`pd_ps.sq`): N = 4, f = 1/4",
        "",
        "| λ | mode | decoding per station | decode time | TPOT | out tok/s | unstable |",
        "|---|---|---|---|---|---|---|",
    ]
    groups = {}
    for r in rows:
        groups.setdefault((r["experiment"], r["prompt"], r["lambda"], r["mode"]), []).append(r)
    for (e, p, lam, mode), g in groups.items():
        if e != "ps":
            continue
        s = [r for r in g if r["stable"]]
        if not s:
            out.append(f"| {lam} | {mode} | | | | | {len(g)} |")
            continue
        out.append(
            f"| {lam} | {mode} | {fmt([r['decoding_per_station'] for r in s])} | {fmt([r['decode_time'] for r in s], 1e3, 1)} | {fmt([r['tpot'] for r in s], 1e3)} | {fmt([r['output_tokens_per_s'] for r in s], 1, 0)} | {len(g) - len(s)} |"
        )
    for prompt, (law, sets, _) in CASES.items():
        extra = "".join(f", `{k} = {v}`" for k, v in sets.items())
        out += [
            "",
            f"## Steps (`pd_batching.sq`): 4 colocated vs 3P/1D, prompt `{law}`{extra}",
            "",
            "TPOT is request-weighted (`tpot`, the mean of each request's (last − first)/(o − 1)) and token-weighted (all decode time over all gaps). "
            "`batch` is the decodes of an iteration that carried any, on a decoding engine, `decoding` the time average of decodes in the running iteration; `prefill`/`decode`/`idle` are the fractions of wall-clock time of the engines named.",
            "",
            "| λ | mode | out tok/s | TTFT | TPOT (req) | TPOT (tok) | response | batch | decoding | engines | prefill | decode | idle | unstable |",
            "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|",
        ]
        for (e, p, lam, mode), g in groups.items():
            if e != "step" or p != prompt:
                continue
            s = [r for r in g if r["stable"]]
            if not s:
                out.append(f"| {lam} | {mode} | | | | | | | | | | | | {len(g)} |")
                continue
            phases = [("E", "E")] if mode == "colocated" else [("P", "P"), ("D", "D")]
            for i, (role, label) in enumerate(phases):
                lead = (
                    f"| {lam} | {mode} | {fmt([r['output_tokens_per_s'] for r in s], 1, 0)} | {fmt([r['ttft'] for r in s], 1e3, 1)} | "
                    f"{fmt([r['tpot'] for r in s], 1e3)} | {fmt([r['tpot_token_weighted'] for r in s], 1e3)} | {fmt([r['response'] for r in s], 1e3, 1)} | "
                    f"{fmt([r['batch_per_step'] for r in s], 1, 2)} | {fmt([r['decoding_per_engine'] for r in s], 1, 2)} | "
                    if i == 0
                    else "| | | | | | | | | | "
                )
                out.append(
                    lead
                    + f"{label} | {fmt([r[f'{role}_prefill_only'] for r in s], 1, 2)} | {fmt([r[f'{role}_decode_only'] for r in s], 1, 2)} | {fmt([r[f'{role}_idle'] for r in s], 1, 2)} | "
                    + (f"{len(g) - len(s)} |" if i == 0 else " |")
                )
    out += [
        "",
        "## Variations of the baseline (`pd_batching.sq`, prompts of 2000 tokens)",
        "",
        "One change at a time; the baseline it changes is the `fixed` table above. TPOT is token-weighted; `ITL p99` is the largest of the decoding engines'. "
        "`admit wait` is the decode engine's admission after the prefill, `transfer` the read after it, both in the TTFT; `P KV` is the tokens the prefill engines hold, leased until read; `preempt/s` counts the decoding engines' preemptions.",
        "",
        "| variation | λ | mode | out tok/s | TTFT | TPOT (tok) | ITL p99 | response | batch | admit wait | transfer | P KV | preempt/s | unstable |",
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for case, (title, *_rest) in VARIATIONS.items():
        first = True
        for (e, p, lam, mode), g in groups.items():
            if e != "variation" or p != case:
                continue
            s = [r for r in g if r["stable"]]
            label = f"`{case}`: {title}" if first else ""
            first = False
            if not s:
                out.append(f"| {label} | {lam} | {mode} | | | | | | | | | | | {len(g)} |")
                continue
            out.append(
                f"| {label} | {lam} | {mode} | {fmt([r['output_tokens_per_s'] for r in s], 1, 0)} | {fmt([r['ttft'] for r in s], 1e3, 1)} | "
                f"{fmt([r['tpot_token_weighted'] for r in s], 1e3)} | {fmt([r['itl_p99'] for r in s], 1e3, 2)} | {fmt([r['response'] for r in s], 1e3, 1)} | "
                f"{fmt([r['batch_per_step'] for r in s], 1, 2)} | {fmt([r['admit_wait'] for r in s], 1e3, 2)} | {fmt([r['transfer'] for r in s], 1e3, 2)} | "
                f"{fmt([r['p_kv_used'] for r in s], 1, 0)} | {fmt([r['preemptions_per_s'] for r in s], 1, 2)} | {len(g) - len(s)} |"
            )
    return "\n".join(out) + "\n"


if __name__ == "__main__":
    main()
