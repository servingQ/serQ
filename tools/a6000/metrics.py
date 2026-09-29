#!/usr/bin/env python3
"""Poll every instance's /metrics once a second: KV usage, running, waiting."""
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
            if line.startswith(WANT):
                name, val = line.rsplit(" ", 1)
                row[f"{p}:{name.split('{')[0].split(':')[1]}"] = float(val)
    out.write(json.dumps(row) + "\n"); out.flush()
    time.sleep(1.0)
