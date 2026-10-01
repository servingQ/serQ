#!/usr/bin/env python3
"""Performance probe for the Lean executable semantics.

    scripts/lean_bench.py small            # tools/oracle/cache_trace.ir.json as is
    scripts/lean_bench.py full [SPACING]   # examples/replay/data/short_base.csv, A100 engine

Writes, under target/lean-bench/<name>/, the IR (`prog.ir.json`, the vLLM
replay program on a unit step clock with the trace inlined: the template is
tools/oracle/cache_trace.ir.json) and the same workload as JSON for
`serq-lean-bench`; runs both `serq run` (Rust) and `serq-lean-bench` (Lean),
times them, and compares every observation (sent, ttft, latency, cached
tokens) per session and turn.
"""
import csv, json, math, os, subprocess, sys, time
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TEMPLATE = ROOT / "tools/oracle/cache_trace.ir.json"
LEAN = ROOT / "lean/.lake/build/bin/serq-lean-bench"
SERQ = ROOT / "target/release/serq"


def sessions_from_csv(path):
    by = defaultdict(list)
    for r in csv.DictReader(open(path)):
        by[int(r["session"])].append((int(r["turn"]), r))
    out = []
    for s in sorted(by):
        turns = []
        for _, r in sorted(by[s]):
            turns.append([[3, float(int(r["new"]))], [4, float(int(r["out"]))],
                          [5, float(math.ceil(float(r["think"])))], [7, float(int(r["forced"]))]])
        out.append({"attrs": [], "turns": turns})
    return out


def build(name, spacing=None):
    ir = json.load(open(TEMPLATE))
    if name == "full":
        ir["arrival"] = {"Sessions": sessions_from_csv(ROOT / "examples/replay/data/short_base.csv")}
        ir["pools"][0]["cap"] = 8010.0 * 16   # A100 KV pool, block 16
        ir["pools"][1]["cap"] = 64.0          # max_num_seqs
        ir["stages"][0]["kind"]["Step"]["budget"] = {"Num": 512.0}
        ir["horizon"] = 10_000_000.0
        if spacing is not None:
            ir["blocks"][2][0]["Run"]["work"]["Binary"][2] = {"Num": float(spacing)}
    out = ROOT / "target/lean-bench" / name
    out.mkdir(parents=True, exist_ok=True)
    (out / "prog.ir.json").write_text(json.dumps(ir))
    spacing_ir = ir["blocks"][2][0]["Run"]["work"]["Binary"][2]["Num"]
    assert spacing_ir == 3.0, "the Lean probe compiles vllmTurn with a spacing of 3"
    pools = [[int(p["cap"]), int(p["block"] or 1), 1 if p["admit_via"] is not None else 0] for p in ir["pools"]]
    step = ir["stages"][0]["kind"]["Step"]
    w = {"pools": pools, "budget": int(step["budget"]["Num"]), "chunk": int(step["chunk"]["Num"]),
         "maxTicks": int(ir["horizon"]),
         "sessions": [[[[int(a), int(v)] for a, v in t] for t in s["turns"]] for s in ir["arrival"]["Sessions"]]}
    (out / "workload.json").write_text(json.dumps(w))
    return out, ir


def run_lean(out):
    t = time.time()
    p = subprocess.run([str(LEAN), str(out / "workload.json")], capture_output=True, text=True, check=True)
    dt = time.time() - t
    obs = defaultdict(list)
    for line in p.stdout.splitlines()[1:]:
        n, s, tm, v = map(int, line.split(","))
        obs[n].append((s, v))
    return dt, p.stderr.strip(), obs


def run_rust(out, ir):
    d = out / "rust"
    t = time.time()
    subprocess.run([str(SERQ), "run", str(out / "prog.ir.json"), "--dump", str(d)], capture_output=True, text=True, check=True)
    dt = time.time() - t
    obs = {}
    for i, name in enumerate(ir["observes"]):
        f = d / f"{name}.csv"
        rows = list(csv.DictReader(open(f))) if f.exists() else []
        obs[i] = [(int(r["session"]), float(r["value"])) for r in rows]
    return dt, obs


def main():
    name = sys.argv[1] if len(sys.argv) > 1 else "small"
    out, ir = build(name)
    tl, info, lean = run_lean(out)
    tr, rust = run_rust(out, ir)
    print(f"{name}: Lean {tl:.2f} s ({info}); Rust {tr:.2f} s")
    ok = True
    for i, nm in enumerate(ir["observes"]):
        a = sorted(lean.get(i, []))
        b = sorted((s, int(round(v))) for s, v in rust[i])
        same = a == b
        ok &= same
        print(f"  {nm:14} Lean {len(a):6}  Rust {len(b):6}  {'identical' if same else 'DIFFER'}")
        if not same:
            diff = [(x, y) for x, y in zip(a, b) if x != y][:5]
            print("    first differences (Lean, Rust):", diff)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
