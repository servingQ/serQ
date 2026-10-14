#!/usr/bin/env python3
"""Build one Harbor task per model under PUB/ from RESULTS/ and ATIF/.
usage: build_tasks.py RESULTS ATIF COLLECT_DIR PUB"""
import shutil
import sys
from pathlib import Path

res, atif, col, pub = (Path(a) for a in sys.argv[1:5])
MODELS = {"llama31-8b": ("meta-llama/Llama-3.1-8B-Instruct", "Llama-3.1-8B-Instruct"),
          "qwen3-8b": ("Qwen/Qwen3-8B", "Qwen3-8B")}
SCRIPTS = ["setup.sh", "gen_dataset.py", "run_model.sh", "run_all.sh", "run_knobs.sh", "run_knobs_all.sh",
           "otlp_sink.py", "metrics.py", "bench_to_atif.py", "convert_all.py", "summarize.py"]

TOML = '''schema_version = "1.3"
artifacts = []

[task]
name = "servingq/{name}"
description = "A100 single-turn serving traces, vLLM v0.31.0, {short} (vllm bench serve)"
keywords = ["serving", "vllm", "single-turn", "trace", "serQ", "atif", "{tag}"]
[[task.authors]]
name = "Jinhwan Suk"
email = "jinhwan.suk@rebellions.ai"

[metadata]

[verifier]
timeout_sec = 600.0
collect = []

[verifier.env]

[agent]
timeout_sec = 600.0

[environment]
network_mode = "public"
build_timeout_sec = 600.0
os = "linux"
mcp_servers = []

[environment.env]

[solution.env]
'''

INSTRUCTION = '''# A100 single-turn serving traces: vLLM v0.31.0, {short}

`/app/data` holds the records of `vllm bench serve` replaying the workload of
serQ's `examples/single-turn/vllm_single_turn.sq` (prompt uniform(1000, 3000)
tokens, output exp(200)+1 tokens, Poisson arrivals) against vLLM v0.31.0
serving `{model}` on one A100-SXM4-40GB (Lambda, 2026-10-13). `runs/sync/` is
the saturation probe (`probe_c<C>.json`, closed loop at C concurrent requests)
and the Poisson sweep (`sweep_f<f>_r<rep>.json`, rate f x lambda_max, three
repetitions); `runs/async/` the same 1.0 x lambda_max point with async
scheduling on; `runs/knob_<setting>/` one engine setting changed at
0.9 x lambda_max (`knee_r<rep>.json`). `data/README.md` (Korean) documents the
setup and every file; `trajectories/` holds every request as an ATIF-v1.7
trajectory.

Write `/app/summary.json` with `lambda_max`, the largest `request_throughput`
over `runs/sync/probe_c*.json`, and for each sweep fraction f the mean over the
three repetitions of `mean_ttft_ms` in `runs/sync/sweep_f<f>_r<rep>.json`:

```json
{{"lambda_max": ..., "ttft_ms": {{"0.2": ..., "0.4": ..., "0.6": ..., "0.8": ..., "0.9": ..., "1.0": ..., "1.2": ...}}}}
```
'''

TEST = '''"""The summary the instruction asks for, recomputed from the records."""
import glob, json, os, statistics

D = "/app/data/runs"


def expected():
    lmax = max(json.load(open(f))["request_throughput"] for f in glob.glob(f"{D}/sync/probe_c*.json"))
    ttft = {}
    for f in ["0.2", "0.4", "0.6", "0.8", "0.9", "1.0", "1.2"]:
        ttft[f] = statistics.mean(json.load(open(f"{D}/sync/sweep_f{f}_r{r}.json"))["mean_ttft_ms"] for r in (1, 2, 3))
    return lmax, ttft


def test_summary():
    assert os.path.exists("/app/summary.json"), "no /app/summary.json"
    got = json.load(open("/app/summary.json"))
    lmax, ttft = expected()
    assert abs(got["lambda_max"] - lmax) <= 0.01 * lmax
    for f, v in ttft.items():
        assert abs(got["ttft_ms"][f] - v) <= 0.01 * max(1.0, v), (f, got["ttft_ms"][f], v)
'''

TEST_SH = '''#!/bin/bash
apt-get update
apt-get install -y curl
curl -LsSf https://astral.sh/uv/0.9.7/install.sh | sh
source $HOME/.local/bin/env
uvx \\
  --with pytest==8.4.1 \\
  --with pytest-json-ctrf==0.3.5 \\
  pytest --ctrf /logs/verifier/ctrf.json /tests/test_outputs.py -rA
if [ $? -eq 0 ]; then
  echo 1 > /logs/verifier/reward.txt
else
  echo 0 > /logs/verifier/reward.txt
fi
'''

DOCKER = '''FROM python:3.12-slim

# The measured records of the A100 single-turn runs (see data/README.md),
# copied in so that an agent can analyse them offline.
WORKDIR /app
COPY data /app/data
'''

for tag, (model, short) in MODELS.items():
    name = f"a100-vllm031-single-turn-{tag}-2026-10"
    t = pub / name
    if t.exists():
        shutil.rmtree(t)
    data = t / "environment" / "data"
    (t / "tests").mkdir(parents=True)
    data.mkdir(parents=True)
    (t / "task.toml").write_text(TOML.format(name=name, short=short, tag=tag))
    (t / "instruction.md").write_text(INSTRUCTION.format(short=short, model=model))
    (t / "tests" / "test_outputs.py").write_text(TEST)
    (t / "tests" / "test.sh").write_text(TEST_SH)
    (t / "environment" / "Dockerfile").write_text(DOCKER)
    shutil.copytree(res / tag, data / "runs")
    (data / "inputs").mkdir()
    for s in (1, 2, 3):
        shutil.copy(res / "data" / f"{tag}-s{s}.jsonl", data / "inputs" / f"{tag}-s{s}.jsonl")
    shutil.copy(res / "host.txt", data / "host.txt")
    (data / "scripts").mkdir()
    for s in SCRIPTS:
        shutil.copy(col / s, data / "scripts" / s)
    # one archive: the registry times out on tens of thousands of files per task
    shutil.make_archive(str(data / "trajectories"), "gztar", root_dir=atif / tag)
    kv = {"llama31-8b": "163 008", "qwen3-8b": "145 344"}[tag]
    lmax = (res / tag / "lambda_max.txt").read_text().strip()
    (data / "README.md").write_text((col / "README_data.md").read_text().format(short=short, model=model, kv=kv, lmax=lmax))
    (t / "README.md").write_text(
        f"# {name}\n\nMeasured single-turn serving records on one A100-SXM4-40GB: vLLM v0.31.0 (release) serving\n"
        f"{model}, replaying serQ's `examples/single-turn/vllm_single_turn.sq` workload with `vllm bench serve`:\n"
        f"a saturation probe, a Poisson sweep (0.2-1.2 x lambda_max, 3 repetitions), one async-scheduling point,\n"
        f"and seven engine settings at the knee. Every request is also an ATIF-v1.7 trajectory. The data is under\n"
        f"`environment/data/` (Korean README inside). servingQ/serQ#589.\n")
    print("built", t)
