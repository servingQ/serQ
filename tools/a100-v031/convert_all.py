#!/usr/bin/env python3
"""Convert every bench result under RESULTS/<tag>/<server dir>/ to ATIF + requests.jsonl.
usage: convert_all.py RESULTS OUT"""
import json
import subprocess
import sys
from pathlib import Path

MODELS = {"llama31-8b": "meta-llama/Llama-3.1-8B-Instruct", "qwen3-8b": "Qwen/Qwen3-8B"}
res, out = Path(sys.argv[1]), Path(sys.argv[2])
here = Path(__file__).parent
for tag, model in MODELS.items():
    for d in sorted((res / tag).iterdir()):
        if not (d / "index.jsonl").exists():
            continue
        for l in open(d / "index.jsonl"):
            name = json.loads(l)["name"]
            if name == "warmup" or not (d / f"{name}.json").exists():
                continue
            run_id = f"{tag}/{d.name}/{name}"
            subprocess.run([sys.executable, str(here / "bench_to_atif.py"), str(d), name, model, run_id,
                            str(out / tag / d.name / name)], check=True)
