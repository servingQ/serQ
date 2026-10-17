#!/usr/bin/env python3
"""Poll /metrics once a second: every unlabelled-or-engine gauge and counter
(no histogram buckets), one JSON line per poll. Histograms are snapshotted
whole by the driver before and after each run.

usage: metrics.py PORT OUT.jsonl
"""
import json
import sys
import time
import urllib.request

port, out = int(sys.argv[1]), open(sys.argv[2], "a")
while True:
    row = {"t": time.time()}
    try:
        txt = urllib.request.urlopen(f"http://127.0.0.1:{port}/metrics", timeout=2).read().decode()
    except Exception:
        txt = ""
    for line in txt.splitlines():
        if not line.startswith("vllm:") or "_bucket" in line:
            continue
        name, v = line.rsplit(" ", 1)
        base = name.split("{")[0]
        if base.endswith(("_sum", "_count", "_created")):
            continue
        key = base[5:]
        if "reason=" in name or "source=" in name:
            key += "{" + name.split("{", 1)[1].split("}")[0].split("model_name")[0].rstrip(",") + "}"
        try:
            row[key] = float(v)
        except ValueError:
            pass
    out.write(json.dumps(row) + "\n")
    out.flush()
    time.sleep(1.0)
