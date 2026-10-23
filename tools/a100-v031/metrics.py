#!/usr/bin/env python3
"""Poll /metrics once a second: every gauge and counter (no histogram
buckets, sums or counts, and no `_info` gauge, whose labels are the engine's
configuration), one JSON line per poll. A series is keyed by its
name less `vllm:` and every label but `model_name`, so the series of one
metric (`reason`, `source`, `sleep_state`, `finished_reason`, ...) stay apart;
a value that is not finite is null. Histograms are snapshotted whole by the
driver before and after each run.

usage: metrics.py PORT OUT.jsonl
"""
import json
import math
import re
import sys
import time
import urllib.request

LABEL = re.compile(r'(\w+)="((?:[^"\\]|\\.)*)"')


def key(name):
    """`vllm:x{engine="0",model_name="m",reason="r"}` -> `x{engine="0",reason="r"}`."""
    base, _, labels = name.partition("{")
    kept = [f'{k}="{v}"' for k, v in LABEL.findall(labels) if k != "model_name"]
    return base[len("vllm:"):] + ("{" + ",".join(kept) + "}" if kept else "")


def row(txt):
    """The gauges and counters of one /metrics text."""
    out = {}
    for line in txt.splitlines():
        if not line.startswith("vllm:"):
            continue
        name, v = line.rsplit(" ", 1)
        if name.split("{")[0].endswith(("_bucket", "_sum", "_count", "_created", "_info")):
            continue
        try:
            x = float(v)
        except ValueError:
            continue
        out[key(name)] = x if math.isfinite(x) else None
    return out


def main():
    port, out = int(sys.argv[1]), open(sys.argv[2], "a")
    while True:
        t = time.time()
        try:
            txt = urllib.request.urlopen(f"http://127.0.0.1:{port}/metrics", timeout=2).read().decode()
        except Exception:
            txt = ""
        out.write(json.dumps({"t": t, **row(txt)}, allow_nan=False) + "\n")
        out.flush()
        time.sleep(1.0)


if __name__ == "__main__":
    main()
