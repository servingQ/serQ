#!/usr/bin/env python3
"""Differential random testing of the Lean semantics against the interpreter.

    scripts/lean_drt.py [N] [--seed S]   # N random cases (default 30)

Each case is the vLLM replay program (examples/replay/vllm_replay.sq on the
IR template tools/oracle/cache_trace.ir.json; the Lean executable
`serq-lean-bench` has it compiled in as `Oracle.vllmTurn`) with a random
engine (KV blocks, request slots, budget, chunk cap, iteration cost on the
step clock or an affine one with natural coefficients) and random explicit
sessions (turns, prompt and output lengths, think times, forced misses).
Both `serq run` and the Lean executable run it; every observation of every
session must agree. A failing case is kept in target/lean-drt/<seed>/ (its
IR, the workload, `lean.csv` and the interpreter's `rust/` dump) with the
command that reruns it, and the script exits 1; a passing case is removed.
The seeds are fixed (0 to N-1 by default), so CI checks the same cases
every time; `--seed S` checks others.
"""
import json, random, shutil, sys, traceback
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lean_bench as lb  # noqa: E402


def case(rng):
    ir = json.load(open(lb.TEMPLATE))
    sessions = []
    longest = 0
    for _ in range(rng.randint(1, 40)):
        turns = []
        for _ in range(rng.randint(1, 6)):
            new = rng.randint(1, 3000)
            out = rng.randint(1, 12)
            longest = max(longest, new + out)
            turns.append([[ir["slot_new"], float(new)], [ir["slot_out"], float(out)],
                          [ir["slot_think"], float(rng.randint(0, 50))],
                          [ir["slot_forced"], float(rng.random() < 0.2)]])
        sessions.append({"attrs": [], "turns": turns})
    ir["arrival"] = {"Sessions": sessions}
    block = 16
    blocks = rng.randint(-(-longest // block) + 1, 4000)  # every request fits alone
    ir["pools"][0]["cap"] = float(blocks * block)
    ir["pools"][1]["cap"] = float(rng.randint(1, 64))
    step = ir["stages"][0]["kind"]["Step"]
    step["budget"] = {"Num": float(rng.choice([16, 64, 256, 512, 1024, 2048, rng.randint(1, 4096)]))}
    cap = float(rng.choice([0, 0, 0, rng.randint(1, 512)]))
    if cap and rng.random() < 0.5:
        # vLLM's rule: the cap only while another request runs or waits for
        # a slot of `reqs` (pool 1), as the vLLM programs' schedules write it
        reqs = {"Call": ["Queued", [{"Pool": {"base": 1, "count": 1, "index": None}}]]}
        test = {"Binary": ["Gt", {"Binary": ["Add", {"Ctx": "Nres"}, reqs]}, {"Num": 1.0}]}
        step["chunk"] = {"Cond": [test, {"Num": cap}, {"Num": 0.0}]}
    else:
        step["chunk"] = {"Num": cap}
    if rng.random() < 0.5:
        cost = [1, 0, 0, 0, 0, 0]
    else:
        cost = [rng.randint(1, 5000), rng.randint(0, 3), rng.randint(0, 60), rng.randint(0, 50),
                rng.randint(0, 3), rng.randint(0, 3)]
    step["cost"] = lb.cost_ir(cost)
    ir["horizon"] = 1e15
    return ir, cost


def run(seed):
    rng = random.Random(seed)
    ir, cost = case(rng)
    out = lb.ROOT / "target/lean-drt" / str(seed)
    out.mkdir(parents=True, exist_ok=True)
    (out / "prog.ir.json").write_text(json.dumps(ir))
    lb.write_workload(out, ir, cost)
    _, info, lean = lb.run_lean(out, keep=out / "lean.csv")
    _, rust = lb.run_rust(out, ir)
    bad = [r for r in lb.compare(ir, lean, rust) if r[3] is not None]
    return ir, info, bad, out


def main():
    args = sys.argv[1:]
    n = int(args[0]) if args and not args[0].startswith("-") else 30
    base = int(args[args.index("--seed") + 1]) if "--seed" in args else 0
    failed = 0
    for seed in range(base, base + n):
        try:
            ir, info, bad, out = run(seed)
        except Exception:
            failed += 1
            print(f"seed {seed}: ERROR (rerun: scripts/lean_drt.py 1 --seed {seed})")
            traceback.print_exc()
            continue
        sessions = len(ir["arrival"]["Sessions"])
        if bad:
            failed += 1
            print(f"seed {seed}: DIFFER ({sessions} sessions; {info}); kept in {out.relative_to(lb.ROOT)}, "
                  f"rerun: scripts/lean_drt.py 1 --seed {seed}")
            for name, la, lr, (sess, x, y) in bad:
                print(f"  {name}: Lean {la}, Rust {lr}; session {sess}: Lean {x} Rust {y}")
        else:
            shutil.rmtree(out)
            print(f"seed {seed}: identical ({sessions} sessions)")
    print(f"{n - failed} of {n} cases identical")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
