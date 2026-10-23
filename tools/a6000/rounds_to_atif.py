#!/usr/bin/env python3
"""Convert a rounds.jsonl of this directory (`pd_replay.py`, one line per
request) into ATIF-v1.7 trajectories, one per session (serQ #589; Harbor
`servingq/a6000-pd-nixl-2026-09`). The model defaults to the one those runs
served (`serve_pd.sh`).

usage: rounds_to_atif.py ROUNDS RUN_ID OUT_DIR [--extra JSON]
"""
import argparse
import json
from collections import defaultdict
from pathlib import Path

ap = argparse.ArgumentParser()
ap.add_argument("rounds")
ap.add_argument("run_id")
ap.add_argument("out")
ap.add_argument("--extra", default="{}", help="root extra (deployment, workload, ...)")
ap.add_argument("--engine", default="vllm")
ap.add_argument("--version", default="unknown")
ap.add_argument("--model", default="Qwen/Qwen3-8B")
a = ap.parse_args()

rows, skipped = [], 0
for l in open(a.rounds):
    try:
        rows.append(json.loads(l))
    except ValueError:
        skipped += 1
by_session = defaultdict(list)
for r in rows:
    by_session[r["session"]].append(r)

out = Path(a.out)
out.mkdir(parents=True, exist_ok=True)
for s, turns in sorted(by_session.items()):
    turns.sort(key=lambda r: r["turn"])
    steps = []
    for r in turns:
        steps.append({
            "step_id": len(steps) + 1,
            "source": "user",
            "message": "",
            "extra": {"turn": r["turn"], "new_tokens": r["new"], "out_requested": r["out"],
                      "forced": r["forced"], "prompt_len": r["prompt_len"]},
        })
        steps.append({
            "step_id": len(steps) + 1,
            "source": "agent",
            "message": "",
            "llm_call_count": 1,
            "metrics": {"prompt_tokens": r["prompt_tokens"],
                        "completion_tokens": r["generated"],
                        "cached_tokens": r["cached_tokens"]},
            "extra": {"serq": {"session": s, "turn": r["turn"], "sent": r["sent"],
                               "first": r["first"], "done": r["done"],
                               "prompt": r["prompt_tokens"], "cached": r["cached_tokens"],
                               "out": r["generated"]},
                      "status": r["status"]},
        })
    traj = {
        "schema_version": "ATIF-v1.7",
        "session_id": a.run_id,
        "trajectory_id": f"{a.run_id}/s{s}",
        "agent": {"name": a.engine, "version": a.version, "model_name": a.model},
        "steps": steps,
        "final_metrics": {
            "total_prompt_tokens": sum(r["prompt_tokens"] for r in turns),
            "total_completion_tokens": sum(r["generated"] for r in turns),
            "total_cached_tokens": sum(r["cached_tokens"] for r in turns),
            "total_steps": len(steps),
        },
        "notes": "message is empty: the source records token counts, not text. "
                 "Times in extra.serq are seconds from the run start."
                 + (f" {skipped} unparsable line(s) of the source were skipped." if skipped else ""),
        "extra": {"run_id": a.run_id, "source": a.rounds, **json.loads(a.extra)},
    }
    (out / f"s{s}.json").write_text(json.dumps(traj, indent=1) + "\n")
print(f"{len(by_session)} trajectories, {len(rows)} requests, {skipped} skipped -> {out}")
