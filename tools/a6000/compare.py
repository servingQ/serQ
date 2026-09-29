#!/usr/bin/env python3
"""Run examples/replay/llmd_pd_replay.seq with the fitted constants and compare its
per-request TTFT with the testbed's rounds.jsonl, request for request."""
import json, os, subprocess, sys, tempfile, statistics as st
SEQ = sys.argv[1]; PROG = sys.argv[2]; ROUNDS = sys.argv[3]; SETS = sys.argv[4:]
meas = {}
for l in open(ROUNDS):
    r = json.loads(l)
    if r.get("first"):
        meas[(r["session"], r["turn"])] = (r["first"] - r["sent"], r.get("cached_tokens", 0), r["prompt_len"])
with tempfile.TemporaryDirectory() as d:
    args = [SEQ, "run", PROG, "--dump", d]
    for s in SETS: args += ["--set", s]
    rep = subprocess.run(args, check=True, capture_output=True, text=True).stdout
    print("\n".join(l for l in rep.splitlines() if l.startswith("run:") or l.startswith("  ttft") or l.startswith("  lease") or l.startswith("  latency")))
    pred = {}; dcached = {}
    for line in open(os.path.join(d, "ttft.csv")).read().splitlines()[1:]:
        t, s, k, v = line.split(","); pred[(int(s), int(k))] = float(v)
    for line in open(os.path.join(d, "d_cached.csv")).read().splitlines()[1:]:
        t, s, k, v = line.split(","); dcached[(int(s), int(k))] = float(v)
keys = sorted(set(meas) & set(pred))
m = [meas[k][0] for k in keys]; p = [pred[k] for k in keys]
print(f"requests matched {len(keys)}: measured mean TTFT {st.mean(m):.3f} s, seQ {st.mean(p):.3f} s; median {st.median(m):.3f} vs {st.median(p):.3f}; p99 {sorted(m)[int(.99*len(m))]:.3f} vs {sorted(p)[int(.99*len(p))]:.3f}")
import math
lr = [math.log(pred[k] / meas[k][0]) for k in keys if meas[k][0] > 0 and pred[k] > 0]
print(f"mean |log ratio| {st.mean(abs(x) for x in lr):.3f}, bias (mean log ratio) {st.mean(lr):+.3f}")
first = [k for k in keys if k[1] == 1]; fol = [k for k in keys if k[1] > 1]
for name, ks in (("first turns", first), ("follow-ups", fol)):
    if ks: print(f"{name}: n={len(ks)} measured {st.mean(meas[k][0] for k in ks):.3f} s, seQ {st.mean(pred[k] for k in ks):.3f} s")
# cached tokens on the decoder: measured vs seQ's d_cached
cm = [meas[k][1] for k in fol if k in dcached]; cp = [dcached[k] for k in fol if k in dcached]
if cm: print(f"decoder cached tokens on follow-ups: measured mean {st.mean(cm):.0f}, seQ {st.mean(cp):.0f}; exact matches {sum(1 for a,b in zip(cm,cp) if abs(a-b)<0.5)}/{len(cm)}")
