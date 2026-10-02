#!/usr/bin/env python3
"""Performance probe for the Lean executable semantics.

    scripts/lean_bench.py small   # tools/oracle/cache_trace.ir.json as is
    scripts/lean_bench.py full    # examples/replay/data/short_base.csv, A100 engine

Writes, under target/lean-bench/<name>/, the IR (`prog.ir.json`, the vLLM
replay program on a unit step clock with the trace inlined: the template is
tools/oracle/cache_trace.ir.json; think times are rounded up to whole
steps) and the same workload as JSON for `serq-lean-bench`; runs both
`serq run` (Rust, `target/release/serq`, built by `cargo build --release`)
and `serq-lean-bench` (Lean), times them, and compares every observation:
for each observation and session, the sequence of (time, value) in the
order they were made. The Lean executable has `Oracle.vllmTurn` compiled
in, so the IR may differ from cache_trace.ir.json only in its pools,
engine and sessions; `build` checks the rest.
"""
import csv, json, math, os, subprocess, sys, time
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TEMPLATE = ROOT / "tools/oracle/cache_trace.ir.json"
LEAN = ROOT / "lean/.lake/build/bin/serq-lean-bench"
SERQ = ROOT / "target/release/serq"


def sessions_from_csv(path, ir):
    by = defaultdict(list)
    for r in csv.DictReader(open(path)):
        by[int(r["session"])].append((int(r["turn"]), r))
    out = []
    for s in sorted(by):
        turns = []
        for _, r in sorted(by[s]):
            turns.append([[ir["slot_new"], float(int(r["new"]))], [ir["slot_out"], float(int(r["out"]))],
                          [ir["slot_think"], float(math.ceil(float(r["think"])))],
                          [ir["slot_forced"], float(int(r["forced"]))]])
        out.append({"attrs": [], "turns": turns})
    return out


def build(name):
    ir = json.load(open(TEMPLATE))
    if name == "full":
        ir["arrival"] = {"Sessions": sessions_from_csv(ROOT / "examples/replay/data/short_base.csv", ir)}
        ir["pools"][0]["cap"] = 8010.0 * 16   # A100 KV pool, block 16
        ir["pools"][1]["cap"] = 64.0          # max_num_seqs
        ir["stages"][0]["kind"]["Step"]["budget"] = {"Num": 512.0}
        ir["horizon"] = 10_000_000.0
    out = ROOT / "target/lean-bench" / name
    out.mkdir(parents=True, exist_ok=True)
    (out / "prog.ir.json").write_text(json.dumps(ir))
    write_workload(out, ir)
    return out, ir


def write_workload(out, ir):
    """The workload JSON of `serq-lean-bench` for the IR: pools, engine,
    the turn and `more` slots, and every session's preset attributes and
    turns. The program is compiled into the executable, so everything else
    must be the template's."""
    template = json.load(open(TEMPLATE))
    for k in ("blocks", "attrs", "observes", "slot_turn", "slot_more", "init", "turn", "session"):
        assert ir[k] == template[k], f"{k} differs from the program compiled into serq-lean-bench"
    pools = [[int(p["cap"]), int(p["block"] or 1), 1 if p["admit_via"] is not None else 0] for p in ir["pools"]]
    step = ir["stages"][0]["kind"]["Step"]
    nat = lambda v: int(v) if v == int(v) else (_ for _ in ()).throw(ValueError(f"{v} is not a natural number"))
    ss = ir["arrival"]["Sessions"]
    w = {"pools": pools, "budget": nat(step["budget"]["Num"]), "chunk": nat(step["chunk"]["Num"]),
         "horizon": nat(ir["horizon"]), "turnSlot": ir["slot_turn"], "moreSlot": ir["slot_more"],
         "init": [[[a, nat(v)] for a, v in s["attrs"]] for s in ss],
         "sessions": [[[[a, nat(v)] for a, v in t] for t in s["turns"]] for s in ss]}
    (out / "workload.json").write_text(json.dumps(w))


def run_lean(out):
    """Seconds, the executable's summary line, and {observation: {session:
    [(time, value)]}} in the order the observations were made."""
    t = time.time()
    p = subprocess.run([str(LEAN), str(out / "workload.json")], capture_output=True, text=True, check=True)
    dt = time.time() - t
    obs = defaultdict(lambda: defaultdict(list))
    for line in p.stdout.splitlines()[1:]:
        n, s, tm, v = map(int, line.split(","))
        obs[n][s].append((tm, v))
    return dt, p.stderr.strip(), obs


def integral(x, what):
    v = float(x)
    if v != int(v):
        raise ValueError(f"{what} {x} is not a whole number of clock units")
    return int(v)


def run_rust(out, ir):
    """Seconds, and the same structure as `run_lean` from `serq run --dump`."""
    d = out / "rust"
    t = time.time()
    subprocess.run([str(SERQ), "run", str(out / "prog.ir.json"), "--dump", str(d)], capture_output=True, text=True, check=True)
    dt = time.time() - t
    obs = {}
    for i, name in enumerate(ir["observes"]):
        f = d / f"{name}.csv"
        rows = list(csv.DictReader(open(f))) if f.exists() else []
        per = defaultdict(list)
        for r in rows:
            per[int(r["session"])].append((integral(r["time"], "time"), integral(r["value"], name)))
        obs[i] = per
    return dt, obs


def compare(ir, lean, rust):
    """Per observation: (name, Lean count, Rust count, first differing
    (session, Lean, Rust) or None). Fails if neither side observed anything."""
    rows = []
    for i, name in enumerate(ir["observes"]):
        a, b = lean.get(i, {}), rust.get(i, {})
        first = None
        for s in sorted(set(a) | set(b)):
            if a.get(s, []) != b.get(s, []):
                x, y = a.get(s, []), b.get(s, [])
                k = next((k for k in range(max(len(x), len(y))) if x[k:k + 1] != y[k:k + 1]), 0)
                first = (s, x[k:k + 3], y[k:k + 3])
                break
        rows.append((name, sum(map(len, a.values())), sum(map(len, b.values())), first))
    if not any(r[1] or r[2] for r in rows):
        raise RuntimeError("no observation on either side")
    return rows


def main():
    name = sys.argv[1] if len(sys.argv) > 1 else "small"
    out, ir = build(name)
    tl, info, lean = run_lean(out)
    tr, rust = run_rust(out, ir)
    print(f"{name}: Lean {tl:.2f} s ({info}); Rust {tr:.2f} s")
    ok = True
    for nm, la, lr, first in compare(ir, lean, rust):
        ok &= first is None
        print(f"  {nm:14} Lean {la:6}  Rust {lr:6}  {'identical' if first is None else 'DIFFER'}")
        if first is not None:
            print(f"    session {first[0]}: Lean {first[1]} Rust {first[2]}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
