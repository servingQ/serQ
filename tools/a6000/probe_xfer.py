#!/usr/bin/env python3
"""Lone requests of growing prompt length through the proxy: TTFT and the
decoder's nixl transfer-time histogram delta per request, for the link's
bandwidth and the step cost. Run with nothing else in flight."""
import json, sys, time, urllib.request
base = sys.argv[1]; dport = sys.argv[2]; out = open(sys.argv[3], "w")

def hist():
    txt = urllib.request.urlopen(f"http://127.0.0.1:{dport}/metrics", timeout=5).read().decode()
    h = {}
    for l in txt.splitlines():
        if l.startswith("vllm:nixl_xfer_time_seconds_bucket") or l.startswith("vllm:nixl_xfer_time_seconds_sum") or l.startswith("vllm:nixl_xfer_time_seconds_count"):
            k = l.split("{")[0].split("_seconds_")[1] + ("_" + l.split('le="')[1].split('"')[0] if 'le="' in l else "")
            h[k] = float(l.rsplit(" ", 1)[1])
    return h

seed = 7000
for n in (512, 1024, 2048, 4096, 8192, 16384, 32000):
    for rep in range(2):
        seed += 1
        ids = [ (seed * 7919 + i * 104729) % 90000 + 1000 for i in range(n) ]
        body = {"model": "Qwen/Qwen3-8B", "prompt": ids, "max_tokens": 16, "temperature": 0, "ignore_eos": True, "stream": True, "stream_options": {"include_usage": True}}
        h0 = hist(); t0 = time.time(); first = None; usage = None
        r = urllib.request.urlopen(urllib.request.Request(f"{base}/v1/completions", data=json.dumps(body).encode(), headers={"Content-Type": "application/json"}), timeout=600)
        for raw in r:
            line = raw.decode().strip()
            if not line.startswith("data:") or line.endswith("[DONE]"): continue
            j = json.loads(line[5:])
            if j.get("choices") and (j["choices"][0].get("text") or j["choices"][0].get("token_ids")) and first is None: first = time.time()
            if j.get("usage"): usage = j["usage"]
        done = time.time(); time.sleep(1.0); h1 = hist()
        rec = {"n": n, "ttft": first - t0 if first else None, "total": done - t0, "usage": usage,
               "xfer_sum": h1.get("sum", 0) - h0.get("sum", 0), "xfer_count": h1.get("count", 0) - h0.get("count", 0)}
        out.write(json.dumps(rec) + "\n"); out.flush(); print(rec)
        time.sleep(2.0)
