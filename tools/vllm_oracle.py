#!/usr/bin/env python
"""Drive the real vLLM v1 scheduler (ref/vllm) on a deterministic scenario
and print, per request, the step of its first token and of its finish, and
the number of preemptions: the oracle for `tests/vllm_oracle.rs`.

Usage: vllm_oracle.py SCENARIO.json [TARGET.json]  (see the Rust test for
the format). Runs inside the testbed venv (`~/vllm-rbln-dynkv/.venv`), whose
rbln platform plugin forces a 512-token block size.

TARGET.json is what `serq target` printed for the scenario's program. Its
configuration must be the scenario's, and when it names the programmable
scheduler (`scheduler_cls`, `tools/serq_vllm.py`) the scheduler is that
class with the program's serve keys: the oracle of `tests/vllm_target_oracle.rs`.
"""
import json
import os
import sys

os.environ.setdefault("VLLM_ALLOW_LONG_MAX_MODEL_LEN", "1")

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "ref", "vllm"))

from tests.v1.core.utils import create_requests, create_scheduler  # noqa: E402
from vllm.v1.outputs import ModelRunnerOutput  # noqa: E402
from vllm.v1.request import RequestStatus  # noqa: E402


def run(sc, target=None):
    s = create_scheduler(
        max_num_batched_tokens=sc["budget"],
        max_num_seqs=sc["max_seqs"],
        block_size=sc["block_size"],
        num_blocks=sc["num_blocks"],
        enable_prefix_caching=sc.get("prefix_caching", False),
        long_prefill_token_threshold=sc.get("chunk", 0),
        max_model_len=sc.get("max_model_len", 8192),
    )
    if target is not None:
        c = target["config"]
        want = {
            "max_num_batched_tokens": sc["budget"],
            "max_num_seqs": sc["max_seqs"],
            "block_size": sc["block_size"],
            "num_gpu_blocks": sc["num_blocks"],
            "long_prefill_token_threshold": sc.get("chunk", 0),
            "enable_prefix_caching": sc.get("prefix_caching", False),
        }
        for k, v in want.items():
            if c[k] != v:
                sys.exit(f"target {k} = {c[k]}, the scenario's is {v}")
        if c.get("scheduler_cls") == "serq_vllm.SerqScheduler":
            from serq_vllm import SerqScheduler

            s.__class__ = SerqScheduler
            s.serq_init(target["serve_by"])
    reqs = []
    for i, r in enumerate(sc["requests"]):
        (req,) = create_requests(
            num_requests=1,
            num_tokens=r["prompt"],
            max_tokens=r["out"],
            block_size=sc["block_size"],
            req_ids=[str(i)],
        )
        # unique prompts so that no two requests share a prefix
        req._all_token_ids = [(i * 7919 + k) % 50000 + 1 for k in range(r["prompt"])]
        req.prompt_token_ids = list(req._all_token_ids)
        req.num_prompt_tokens = r["prompt"]
        reqs.append((r.get("arrive", 0), req))
    first = {}
    done = {}
    preempt = 0
    step = 0
    pending = sorted(reqs, key=lambda x: x[0])
    while step < sc.get("max_steps", 10000):
        while pending and pending[0][0] <= step:
            s.add_request(pending.pop(0)[1])
        if not s.has_requests():
            break
        step += 1
        out = s.schedule()
        preempt += len(out.preempted_req_ids or [])
        ids = list(out.num_scheduled_tokens.keys())
        sampled = []
        for rid in ids:
            req = s.requests[rid]
            # a request that has caught up with its prompt produces a token
            if req.num_computed_tokens >= req.num_prompt_tokens:
                sampled.append([1])
                if rid not in first:
                    first[rid] = step
            else:
                sampled.append([])
        mro = ModelRunnerOutput(
            req_ids=ids,
            req_id_to_index={rid: i for i, rid in enumerate(ids)},
            sampled_token_ids=sampled,
            logprobs=None,
            prompt_logprobs_dict={},
            pooler_output=[],
        )
        eco = s.update_from_output(out, mro)
        for outs in eco.values():
            for o in outs.outputs:
                if o.finish_reason is not None:
                    done[o.request_id] = step
    return {
        "first": first,
        "done": done,
        "preemptions": preempt,
        "steps": step,
        "config": {
            "budget": s.max_num_scheduled_tokens,
            "max_seqs": s.max_num_running_reqs,
            "chunk": s.scheduler_config.long_prefill_token_threshold,
            "block_size": s.block_size,
            "num_blocks": s.kv_cache_config.num_blocks,
        },
    }


if __name__ == "__main__":
    sc = json.load(open(sys.argv[1]))
    target = json.load(open(sys.argv[2])) if len(sys.argv) > 2 else None
    print(json.dumps(run(sc, target)))
