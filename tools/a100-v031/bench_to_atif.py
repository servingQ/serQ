#!/usr/bin/env python3
"""Turn one `vllm bench serve --save-detailed` result into ATIF-v1.7
trajectories (one per request: a single-turn session) plus requests.jsonl in
the serQ cache_trace columns, joined with the server spans by request id.

usage: bench_to_atif.py SERVER_DIR NAME MODEL RUN_ID OUT_DIR
  SERVER_DIR holds NAME.json (bench), index.jsonl and spans.jsonl
"""
import json
import sys
from pathlib import Path


def convert(d, name, model, run_id, out):
    """Write OUT/trajectories/s<i>.json and OUT/requests.jsonl; return (requests, spans joined)."""
    res = json.loads((d / f"{name}.json").read_text())
    idx = next(r for r in map(json.loads, (d / "index.jsonl").read_text().splitlines()) if r["name"] == name)
    # the bench input: index.jsonl names it since common.sh; before, it was the
    # seed's. build_tasks.py ships data/<file> as the task's inputs/<file>.
    data = idx.get("data") or f"data/{run_id.split('/')[0]}-s{idx['seed']}.jsonl"
    data = f"inputs/{Path(data).name}"

    # a span's id is cmpl-<request id prefix><i>-0, the prefix ending in NAME/
    spans = {}
    if (d / "spans.jsonl").exists():
        for l in (d / "spans.jsonl").read_text().splitlines():
            s = json.loads(l)
            rid = str(s["attrs"].get("gen_ai.request.id", ""))
            if name + "/" in rid:
                spans[rid.split(name + "/", 1)[1].split("-")[0]] = s

    t0 = min(res["start_times"]) if res["start_times"] else 0.0
    (out / "trajectories").mkdir(parents=True, exist_ok=True)
    rows = []
    for i, (n, o, ttft, lat, st, err, itl) in enumerate(zip(
            res["input_lens"], res["output_lens"], res["ttfts"], res["latencies"],
            res["start_times"], res["errors"], res["itls"])):
        sp = spans.get(str(i))
        a = sp["attrs"] if sp else {}
        sent = st - t0
        row = {"session": i, "turn": 1, "sent": sent,
               "first": sent + ttft if not err else None, "done": sent + lat if not err else None,
               "prompt": a.get("gen_ai.usage.prompt_tokens", n), "cached": None, "out": o}
        server = {k.split(".")[-1]: v for k, v in a.items() if k.startswith("gen_ai.latency")} if sp else None
        rows.append({**row, "status": "ok" if not err else err, "server": server})
        steps = [
            {"step_id": 1, "source": "user", "message": "", "extra": {"turn": 1, "new_tokens": n}},
            {"step_id": 2, "source": "agent", "message": "", "llm_call_count": 1,
             "metrics": {"prompt_tokens": row["prompt"], "completion_tokens": o},
             "extra": {"serq": row, "server": server, "itl": itl, "status": "ok" if not err else err}},
        ]
        traj = {
            "schema_version": "ATIF-v1.7", "session_id": run_id, "trajectory_id": f"{run_id}/s{i}",
            "agent": {"name": "vllm", "version": "v0.31.0", "model_name": model},
            "steps": steps,
            "final_metrics": {"total_prompt_tokens": row["prompt"], "total_completion_tokens": o, "total_steps": 2},
            "notes": "Single-turn request replayed by vllm bench serve. message is empty: prompts are words of the "
                     "model's vocabulary, in the bench input extra.input (under the task's data/). Times in "
                     "extra.serq are seconds from the first request.",
            "extra": {"run_id": run_id, "bench": f"{name}.json", "input": data, "rate": idx["rate"],
                      "max_concurrency": idx["max_concurrency"], "seed": idx["seed"]},
        }
        (out / "trajectories" / f"s{i}.json").write_text(json.dumps(traj) + "\n")
    with open(out / "requests.jsonl", "w") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
    return len(rows), len(spans)


if __name__ == "__main__":
    d, name, model, run_id, out = Path(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4], Path(sys.argv[5])
    n, s = convert(d, name, model, run_id, out)
    print(f"{name}: {n} requests, {s} spans joined -> {out}")
