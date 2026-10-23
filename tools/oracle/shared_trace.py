#!/usr/bin/env python3
"""Write the shared-system-prompt scenario (`tools/oracle/shared_trace.*`).

Three sessions of three turns, as `cache_trace`, each prompt beginning with
the same system prompt of S tokens: `shared_trace.csv` for serQ (session,
turn, new, out, think, forced; `new` is the whole prompt, as in the replay
traces) and `shared_trace.jsonl` with the token ids for the real scheduler
(`tools/vllm_replay_oracle.py`). A follow-up prompt is the previous prompt
and new text: the generated tokens are not in it. Deterministic.

    python3 tools/oracle/shared_trace.py
    VLLM_TARGET_DEVICE=cpu VLLM_PLUGINS= python \\
        tools/vllm_replay_oracle.py --trace tools/oracle/shared_trace.jsonl \\
        --spacing 5 --budget 64 --max-seqs 4 --blocks 30 --c-step 1 --a 0 --b 0 \\
        --c0 0 --omega 0 --beta 0 --cost sum --d 0 --out tools/oracle/shared_trace.out.csv
"""
import json
import pathlib

S = 64                 # the system prompt's tokens (4 blocks of 16)
SPACING = 5            # seconds between the sessions' first turns
OUT, THINK = 3, 2.0
# each session's prompt lengths past the system prompt, turn by turn
OWN = [[100, 150, 210], [120, 180, 230], [90, 140, 200]]

DIR = pathlib.Path(__file__).resolve().parent
system = list(range(100, 100 + S))
rows, sessions = [], []
for i, lengths in enumerate(OWN):
    text = list(range(10_000 * (i + 1), 10_000 * (i + 1) + max(lengths)))
    requests = []
    for k, n in enumerate(lengths):
        rows.append(f"{i},{k + 1},{S + n},{OUT},{THINK if k + 1 < len(lengths) else 0.0},0")
        requests.append({"in": S + n, "out": OUT, "think_time": 0.0 if k == 0 else THINK,
                         "prompt_ids": system + text[:n]})
    sessions.append({"id": f"s{i}", "arrive": i * SPACING, "requests": requests})
(DIR / "shared_trace.csv").write_text("session,turn,new,out,think,forced\n" + "\n".join(rows) + "\n")
(DIR / "shared_trace.jsonl").write_text("".join(json.dumps(s) + "\n" for s in sessions))
