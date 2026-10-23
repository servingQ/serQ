#!/usr/bin/env python3
"""Poll every instance's /metrics once a second: KV usage, running, waiting.

A series is kept by its exact name. The records here (`*_metrics.jsonl`) were
polled by name prefix, so `vllm:num_requests_waiting` also caught
`num_requests_waiting_by_reason`, whose series (one per `reason`) all went to
one key, `<port>:num_requests_waiting_by_reason`: the last one printed. Read
`<port>:num_requests_waiting` instead.
"""
import json, sys, time, urllib.request
ports = [int(p) for p in sys.argv[1].split(",")]; out = open(sys.argv[2], "w"); t0 = time.time()
WANT = ("vllm:kv_cache_usage_perc", "vllm:num_requests_running", "vllm:num_requests_waiting", "vllm:gpu_cache_usage_perc")
while True:
    row = {"t": round(time.time() - t0, 2)}
    for p in ports:
        try:
            txt = urllib.request.urlopen(f"http://127.0.0.1:{p}/metrics", timeout=2).read().decode()
        except Exception:
            continue
        for line in txt.splitlines():
            name, _, val = line.rpartition(" ")
            if name.split("{")[0] in WANT:
                row[f"{p}:{name.split('{')[0].split(':')[1]}"] = float(val)
    out.write(json.dumps(row) + "\n"); out.flush()
    time.sleep(1.0)
