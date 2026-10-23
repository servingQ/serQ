#!/usr/bin/env python3
"""Convert every bench result under RESULTS/<tag>/<server dir>/ to ATIF + requests.jsonl.
A run whose exit code in index.jsonl is not 0 is reported and left out.
usage: convert_all.py RESULTS OUT"""
import json
import subprocess
import sys
from pathlib import Path

# tag -> model, as common.sh's MODELS
MODELS = {"llama31-8b": "meta-llama/Llama-3.1-8B-Instruct", "qwen3-8b": "Qwen/Qwen3-8B"}


def main(res, out):
    here = Path(__file__).parent
    failed = 0
    for tag, model in MODELS.items():
        for d in sorted((res / tag).iterdir()):
            if not (d / "index.jsonl").exists():
                continue
            for l in open(d / "index.jsonl"):
                r = json.loads(l)
                name = r["name"]
                if name == "warmup":
                    continue
                if r["rc"] != 0 or not (d / f"{name}.json").exists():
                    print(f"left out {tag}/{d.name}/{name}: rc={r['rc']}", file=sys.stderr)
                    failed += 1
                    continue
                run_id = f"{tag}/{d.name}/{name}"
                subprocess.run([sys.executable, str(here / "bench_to_atif.py"), str(d), name, model, run_id,
                                str(out / tag / d.name / name)], check=True)
    return failed


if __name__ == "__main__":
    sys.exit(1 if main(Path(sys.argv[1]), Path(sys.argv[2])) else 0)
