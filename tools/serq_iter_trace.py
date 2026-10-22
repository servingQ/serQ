#!/usr/bin/env python3
"""The iterations of a serQ run, one line each, in the form
`ORACLE_TRACE_ITER` prints the steps of vLLM's scheduler
(`tools/vllm_replay_oracle.py`), so the two can be diffed line by line.

    serq run P.sq --workload W.sq --log LOG.json
    python3 tools/serq_iter_trace.py LOG.json --block 16

It reads the log's `iter` events (`docs/reference/log.md`). A line is
`ITER t s:turn:mn ... | used U cached C`: per request of the batch, in its
order, the session's serial, its turn, its mode (`p` prefill, `d` decode)
and its tokens (the event's `each`); then the engine's memory in blocks of
`--block` units, left out for an engine with no memory. The turn is the
session's last `turn` (a leg takes its session's when it is forked), the
mode its request's last `status`. Every engine's iterations are listed, in
time order.

It prints what the `SERQ_TRACE_ITER` switch printed until #728, line for
line, but for two things the switch took from the program: an engine with
no memory had ` | used 0 cached 0`, and each engine's memory was in its own
pool's blocks, where `--block` is one for all.
"""
import argparse
import json
import sys

LOG_VERSION = 0  # src/engine/log.rs


def num(x):
    """A number as serQ prints one: a whole one without its fraction."""
    x = float(x)
    return str(int(x)) if x.is_integer() else repr(x)


def iter_lines(log, block=1.0):
    turn, mode, lines = {}, {}, []
    for e in log["events"]:
        who = (e.get("s"), e.get("leg"))
        if e["ev"] == "turn":
            turn[who] = e["turn"]
        elif e["ev"] == "fork":
            turn[who] = turn.get((e["s"], None), 0)
        elif e["ev"] == "status":
            mode[who] = e.get("mode")
        elif e["ev"] == "iter":
            parts = []
            for r, n in zip(e["reqs"], e["each"], strict=True):
                w = (r[0], r[1] if len(r) > 1 else None)
                m = "d" if mode.get(w) == "decode" else "p"
                parts.append(f"{r[0]}:{turn.get(w, 0)}:{m}{num(n)}")
            line = f"ITER {e['t']:.4f} " + " ".join(parts)
            if "used" in e:
                line += f" | used {num(e['used'] / block)} cached {num(e['cached'] / block)}"
            lines.append(line)
    return lines


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("log", help="the log of `serq run --log`; - reads it from stdin")
    ap.add_argument(
        "--block",
        type=float,
        default=1.0,
        help="the units of a block of the engine's memory (its pool's `block`); 1 prints units",
    )
    args = ap.parse_args()
    if args.log == "-":
        log = json.load(sys.stdin)
    else:
        with open(args.log) as f:
            log = json.load(f)
    if log.get("version") != LOG_VERSION:
        sys.exit(f"{args.log}: log version {log.get('version')}, this reads {LOG_VERSION}")
    for line in iter_lines(log, args.block):
        print(line)
    # a truncated log lacks the iterations after its last event: a diff of
    # it would end early and look clean
    if log.get("truncated"):
        sys.exit(f"{args.log}: the log is truncated (its event cap); the later iterations are missing")


if __name__ == "__main__":
    main()
