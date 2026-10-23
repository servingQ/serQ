#!/usr/bin/env python3
"""One line per bench result: throughput, TTFT mean/p99, TPOT, failures.
usage: summarize.py RESULT.json..."""
import json
import sys

for f in sys.argv[1:]:
    r = json.load(open(f))
    print(f"{f:45s} thr {r['request_throughput']:5.2f}  ttft {r['mean_ttft_ms']:7.0f} {r['p99_ttft_ms']:7.0f}  "
          f"tpot {r['mean_tpot_ms']:5.1f}  e2e {r['mean_e2el_ms']:7.0f}  done {r['completed']}  fail {r.get('failed')}")
