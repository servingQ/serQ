#!/usr/bin/env python
"""Replay a trace through the real vLLM v1 scheduler and KV-cache manager
(`ref/vllm`) with a time model instead of a GPU: the diagnostic that
separates *semantics* from *timing* when serQ and a measured run disagree.

Each scheduler step lasts
    c_step + max(omega + beta * decode_kv, a * ntok + b * sum_chunks n (K + n/2))
seconds (the cost expression of `examples/replay/vllm_replay.sq`), the
requests carry the trace's real token ids (so prefix hashing is exact), a
request enters the scheduler `c0` after it is sent, session i sends its
first turn at i * spacing and turn k+1 `think` seconds after turn k
finishes.

    VLLM_PLUGINS= python vllm_replay_oracle.py --trace data/exp/traces/short_base.jsonl \
        --spacing 3.5 --out /tmp/replay.csv

Output CSV: session,turn,sent,first,done,prompt,cached,out
Runs in the testbed venv (`~/vllm-rbln-dynkv/.venv`) with `VLLM_PLUGINS=`.
"""
import argparse
import heapq
import json
import os
import sys

os.environ.setdefault("VLLM_ALLOW_LONG_MAX_MODEL_LEN", "1")
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "ref", "vllm"))

from tests.v1.core.utils import create_scheduler  # noqa: E402
from vllm.sampling_params import SamplingParams  # noqa: E402
from vllm.utils.hashing import sha256  # noqa: E402
from vllm.v1.core.kv_cache_utils import get_request_block_hasher, init_none_hash  # noqa: E402
from vllm.v1.outputs import ModelRunnerOutput  # noqa: E402
from vllm.v1.request import Request  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--trace", required=True)
    ap.add_argument("--spacing", type=float, required=True)
    ap.add_argument("--forced", action="store_true", help="ignored: the derived traces carry the nonce in prompt_ids")
    ap.add_argument("--budget", type=int, default=512)
    ap.add_argument("--max-seqs", type=int, default=64)
    ap.add_argument("--block", type=int, default=16)
    ap.add_argument("--blocks", type=int, default=8010, help="allocatable blocks (the null block is added)")
    ap.add_argument("--a", type=float, default=6.7975e-5)
    ap.add_argument("--b", type=float, default=4.3814e-9)
    ap.add_argument("--c0", type=float, default=0.02524)
    ap.add_argument("--omega", type=float, default=0.0196)
    ap.add_argument("--beta", type=float, default=1e-7)
    ap.add_argument("--c-step", type=float, default=0.01)
    ap.add_argument("--cost", default="max", choices=["max", "sum"],
                    help="sum: c_step + d ndec + beta kvb + a npre + b attn (the A100 step fit)")
    ap.add_argument("--d", type=float, default=0.0)
    ap.add_argument("--max-sessions", type=int, default=0)
    ap.add_argument("--out", required=True)
    ap.add_argument("--pin", action="store_true", help="hypothesis H-pin: pin a waiting request's prefix")
    args = ap.parse_args()
    if args.pin:
        sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "scripts", "exp", "lambda", "steptrace"))
        from pinpatch import apply_pin
        from vllm.v1.core.sched.scheduler import Scheduler as _Sch
        apply_pin(_Sch)

    sessions = [json.loads(l) for l in open(args.trace) if l.strip()]
    if args.max_sessions:
        sessions = sessions[: args.max_sessions]
    s = create_scheduler(
        max_num_batched_tokens=args.budget,
        max_num_seqs=args.max_seqs,
        block_size=args.block,
        num_blocks=args.blocks + 1,
        enable_prefix_caching=True,
        max_model_len=40960,
    )
    init_none_hash(sha256)
    owner = {}  # block_id -> request id (last holder)
    trace_ev = bool(os.environ.get("ORACLE_TRACE_EVICT"))
    if trace_ev:
        bp = s.kv_cache_manager.block_pool
        orig = bp._maybe_evict_cached_block

        def logged(block):
            had = block.block_hash is not None
            r = orig(block)
            if had and r:
                print(f"EVICT {now_ref[0]:.4f} {owner.get(block.block_id, '?')}", file=sys.stderr)
            return r

        bp._maybe_evict_cached_block = logged
    now_ref = [0.0]
    hasher = get_request_block_hasher(args.block, sha256)

    events = []  # (time, seq, session, turn)
    seq = 0
    for i in range(len(sessions)):
        t_arr = sessions[i].get("arrive", i * args.spacing)
        heapq.heappush(events, (t_arr + args.c0, seq, i, 0))
        seq += 1
    sent = {}
    rows = {}
    now = 0.0
    while events or s.has_requests():
        while events and events[0][0] <= now:
            t, _, i, k = heapq.heappop(events)
            q = sessions[i]["requests"][k]
            ids = list(q["prompt_ids"])  # a forced turn's ids already start with its nonce
            rid = f"{i}:{k}"
            req = Request(
                request_id=rid,
                prompt_token_ids=ids,
                sampling_params=SamplingParams(ignore_eos=True, max_tokens=max(1, q["out"])),
                pooling_params=None,
                block_hasher=hasher,
            )
            s.add_request(req)
            sent[rid] = t - args.c0
            rows[rid] = dict(session=i, turn=k + 1, sent=t - args.c0, first=None, done=None,
                             prompt=len(ids), cached=None, out=q["out"])
        if not s.has_requests():
            now = events[0][0]
            continue
        now_ref[0] = now
        out = s.schedule()
        if trace_ev:
            for rid in out.num_scheduled_tokens:
                for b in s.kv_cache_manager.get_block_ids(rid)[0]:
                    owner[b] = rid
        if out.total_num_scheduled_tokens == 0:
            # nothing schedulable: wait for the next send (or stop on a deadlock)
            if not events:
                if s.has_unfinished_requests() if hasattr(s, "has_unfinished_requests") else (s.waiting or s.running):
                        print(f"deadlock at {now:.2f} s: {len(s.waiting)} waiting, {len(s.running)} running", file=sys.stderr)
                break
            now = max(now, events[0][0])
            continue
        for nr in out.scheduled_new_reqs:
            r = rows[nr.req_id]
            if r["cached"] is None:
                r["cached"] = nr.num_computed_tokens
        ids = list(out.num_scheduled_tokens.keys())
        # the cost expression of vllm_replay.sq, in the same quantities:
        # kv_decode / kv_prefill = KV units held (allocated blocks) by the
        # scheduled decoding / prefilling requests, prefilled = prefill tokens
        # scheduled (the variables below keep the old short names)
        ntok = 0
        npre = 0
        kvb = 0
        kvp = 0
        sampled = []
        for rid in ids:
            req = s.requests[rid]
            n = out.num_scheduled_tokens[rid]
            ntok += n
            done_after = req.num_computed_tokens  # already advanced by schedule()
            held = len(s.kv_cache_manager.get_block_ids(rid)[0]) * args.block
            if done_after - n < req.num_prompt_tokens:
                npre += n
                kvp += held
            else:
                kvb += held
            sampled.append([1] if done_after >= req.num_prompt_tokens else [])
        attn = 0.0
        for rid in ids:
            req = s.requests[rid]
            n = out.num_scheduled_tokens[rid]
            k0 = req.num_computed_tokens - n
            if k0 < req.num_prompt_tokens:
                attn += n * (k0 + n / 2)
        if args.cost == "sum":
            ndec = sum(1 for rid in ids
                       if s.requests[rid].num_computed_tokens - out.num_scheduled_tokens[rid]
                       >= s.requests[rid].num_prompt_tokens)
            dt = args.c_step + args.d * ndec + args.beta * kvb + args.a * npre + args.b * attn
        else:
            dt = args.c_step + max(args.omega + args.beta * kvb, args.a * ntok + args.b * npre * (kvp - npre / 2))
        if os.environ.get("ORACLE_TRACE_ITER"):
            parts = []
            for rid in ids:
                req = s.requests[rid]
                n = out.num_scheduled_tokens[rid]
                i, k = rid.split(":")
                mode = "p" if req.num_computed_tokens - n < req.num_prompt_tokens else "d"
                parts.append(f"{i}:{int(k) + 1}:{mode}{n}")
            bp = s.kv_cache_manager.block_pool
            free = bp.get_num_free_blocks()
            cached = sum(1 for b in bp.blocks[1:] if b.ref_cnt == 0 and b.block_hash is not None)
            used = len(bp.blocks) - 1 - free
            print(f"ITER {now:.4f} " + " ".join(parts) + f" | used {used} cached {cached}", file=sys.stderr)
        now += dt
        mro = ModelRunnerOutput(
            req_ids=ids,
            req_id_to_index={rid: j for j, rid in enumerate(ids)},
            sampled_token_ids=sampled,
            logprobs=None,
            prompt_logprobs_dict={},
            pooler_output=[],
        )
        for rid, smp in zip(ids, sampled):
            if smp and rows[rid]["first"] is None:
                rows[rid]["first"] = now
        eco = s.update_from_output(out, mro)
        for outs in eco.values():
            for o in outs.outputs:
                if o.finish_reason is not None:
                    r = rows[o.request_id]
                    r["done"] = now
                    i, k = map(int, o.request_id.split(":"))
                    reqs = sessions[i]["requests"]
                    if k + 1 < len(reqs):
                        heapq.heappush(events, (now + reqs[k + 1].get("think_time", 0.0) + args.c0, seq, i, k + 1))
                        seq += 1
    with open(args.out, "w") as f:
        f.write("session,turn,sent,first,done,prompt,cached,out\n")
        for r in sorted(rows.values(), key=lambda r: (r["session"], r["turn"])):
            f.write(",".join(str(r[c]) for c in ["session", "turn", "sent", "first", "done", "prompt", "cached", "out"]) + "\n")
    print(f"{args.out}: {len(rows)} requests, end {now:.1f} s", file=sys.stderr)


if __name__ == "__main__":
    main()
