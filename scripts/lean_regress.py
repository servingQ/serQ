#!/usr/bin/env python3
"""The interpreter's answers for the Lean regressions in tests/lean-regress/.

    scripts/lean_regress.py --bless   # write <name>.ir.json and <name>.out.json
    scripts/lean_regress.py           # check that they are current

Each `<name>.sq` is compiled (`serq ir`) and its batch arrival replaced by
the explicit sessions of `<name>.sessions.json` (their preset attributes),
which gives `<name>.ir.json`, the IR the Lean theorem is generated from
(scripts/gen_lean_oracle.py writes lean/Serq/Regress.lean). `serq run` on
that IR gives `<name>.out.json`: every observation as (session, value) in
session order and, within a session, in the order made (what
`Exec.observed` returns), and the number of preemptions. Needs
`target/release/serq` (`cargo build --release`).
"""
import csv, json, subprocess, sys, tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DIR = ROOT / "tests" / "lean-regress"
SERQ = ROOT / "target" / "release" / "serq"


def ir_of(sq):
    ir = json.loads(subprocess.run([str(SERQ), "ir", str(sq), "--instance", str(sq.parent / "instances" / sq.stem / "default.sq")], capture_output=True, text=True, check=True).stdout)
    sessions = json.load(open(sq.with_suffix(".sessions.json")))
    slot = {n: i for i, n in enumerate(ir["attrs"])}
    ir["arrival"] = {"Sessions": [{"attrs": [[slot[k], float(v)] for k, v in s.items()], "turns": []}
                                  for s in sessions]}
    return ir


def answers(ir):
    with tempfile.TemporaryDirectory() as d:
        p = Path(d) / "prog.ir.json"
        p.write_text(json.dumps(ir))
        rep = json.loads(subprocess.run([str(SERQ), "run", str(p), "--json", "--dump", str(Path(d) / "dump")],
                                        capture_output=True, text=True, check=True).stdout)
        obs = {}
        for name in ir["observes"]:
            f = Path(d) / "dump" / f"{name}.csv"
            rows = list(csv.DictReader(open(f))) if f.exists() else []
            vals = [(int(r["session"]), float(r["value"])) for r in rows]
            if any(v != int(v) for _, v in vals):
                raise ValueError(f"{name}: a value that is not a natural number")
            obs[name] = sorted(((s, int(v)) for s, v in vals), key=lambda x: x[0])
    return {"observes": obs, "preemptions": sum(p["preemptions"] for p in rep["pools"])}


def main():
    bless = "--bless" in sys.argv
    stale = []
    for sq in sorted(DIR.glob("*.sq")):
        ir = ir_of(sq)
        out = answers(ir)
        for path, val in ((sq.with_suffix(".ir.json"), ir), (sq.with_suffix(".out.json"), out)):
            txt = json.dumps(val, indent=1, sort_keys=True) + "\n"
            if bless:
                path.write_text(txt)
            elif not path.exists() or path.read_text() != txt:
                stale.append(path.name)
    if stale:
        print("STALE:", ", ".join(stale), "(scripts/lean_regress.py --bless, then scripts/gen_lean_oracle.py)")
        sys.exit(1)
    print("wrote" if bless else "current:", ", ".join(p.stem for p in sorted(DIR.glob("*.sq"))))


if __name__ == "__main__":
    main()
