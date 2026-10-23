#!/usr/bin/env python3
"""metrics.py's keys and bench_to_atif.py's join, on made-up input, and the
model list that common.sh and convert_all.py each keep (run by
scripts/check_rust.sh; none of it needs vLLM or a GPU)."""

import json
import tempfile
import unittest
from pathlib import Path

import bench_to_atif
import convert_all
import metrics

HERE = Path(__file__).parent

M = 'model_name="/m/Llama-3.1-8B-Instruct"'
# as prometheus_client prints them: labels sorted by name
EXPOSITION = f"""# HELP vllm:num_requests_running Number of requests in model execution batches.
vllm:num_requests_running{{engine="0",{M}}} 3.0
vllm:num_requests_waiting_by_reason{{engine="0",{M},reason="capacity"}} 7.0
vllm:num_requests_waiting_by_reason{{engine="0",{M},reason="deferred"}} 0.0
vllm:prompt_tokens_by_source_total{{engine="0",{M},source="local_compute"}} 1000.0
vllm:prompt_tokens_by_source_total{{engine="0",{M},source="local_cache_hit"}} 50.0
vllm:prompt_tokens_by_source_created{{engine="0",{M},source="local_compute"}} 1.79e+09
vllm:engine_sleep_state{{engine="0",{M},sleep_state="awake"}} 1.0
vllm:engine_sleep_state{{engine="0",{M},sleep_state="discard_all"}} 0.0
vllm:request_success_total{{engine="0",finished_reason="length",{M}}} 76.0
vllm:e2e_request_latency_seconds_bucket{{engine="0",le="+Inf",{M}}} 76.0
vllm:e2e_request_latency_seconds_sum{{engine="0",{M}}} 12.5
vllm:e2e_request_latency_seconds_count{{engine="0",{M}}} 76.0
vllm:kv_cache_usage_perc{{engine="0",{M}}} NaN
vllm:cache_config_info{{block_size="16",engine="0",num_gpu_blocks="10188"}} 1.0
"""


class Metrics(unittest.TestCase):
    def test_every_series_keeps_its_labels_but_model_name(self):
        self.assertEqual(metrics.row(EXPOSITION), {
            'num_requests_running{engine="0"}': 3.0,
            'num_requests_waiting_by_reason{engine="0",reason="capacity"}': 7.0,
            'num_requests_waiting_by_reason{engine="0",reason="deferred"}': 0.0,
            'prompt_tokens_by_source_total{engine="0",source="local_compute"}': 1000.0,
            'prompt_tokens_by_source_total{engine="0",source="local_cache_hit"}': 50.0,
            'engine_sleep_state{engine="0",sleep_state="awake"}': 1.0,
            'engine_sleep_state{engine="0",sleep_state="discard_all"}': 0.0,
            'request_success_total{engine="0",finished_reason="length"}': 76.0,
            'kv_cache_usage_perc{engine="0"}': None,
        })

    def test_unlabelled(self):
        self.assertEqual(metrics.key("vllm:x"), "x")

    def test_a_poll_is_strict_json(self):
        json.loads(json.dumps(metrics.row(EXPOSITION), allow_nan=False))


class BenchToAtif(unittest.TestCase):
    def test_requests_join_their_spans_by_id(self):
        with tempfile.TemporaryDirectory() as tmp:
            d, out = Path(tmp) / "knob_B512", Path(tmp) / "out"
            d.mkdir()
            (d / "knee_r1.json").write_text(json.dumps({
                "input_lens": [1001, 2001], "output_lens": [5, 7], "ttfts": [0.1, 0.2],
                "latencies": [0.5, 0.9], "start_times": [100.0, 100.25], "errors": ["", ""],
                "itls": [[0.1] * 4, [0.1] * 6]}))
            (d / "index.jsonl").write_text(
                json.dumps({"name": "warmup", "seed": 0, "rate": "inf", "max_concurrency": "4"}) + "\n"
                + json.dumps({"name": "knee_r1", "data": "data/llama31-8b-s1.jsonl", "seed": 1,
                              "rate": "3.3479", "max_concurrency": ""}) + "\n")
            span = {"name": "llm_request", "attrs": {
                "gen_ai.request.id": "cmpl-llama31-8b/knob_B512/knee_r1/1-0",
                "gen_ai.usage.prompt_tokens": 2001, "gen_ai.latency.time_in_queue": 0.05}}
            other = {"name": "llm_request", "attrs": {"gen_ai.request.id": "cmpl-llama31-8b/knob_B512/warmup/1-0"}}
            startup = {"name": "Load weights", "attrs": {}}
            (d / "spans.jsonl").write_text("".join(json.dumps(s) + "\n" for s in (startup, other, span)))

            self.assertEqual(bench_to_atif.convert(d, "knee_r1", "m", "llama31-8b/knob_B512/knee_r1", out), (2, 1))
            rows = [json.loads(l) for l in (out / "requests.jsonl").read_text().splitlines()]
            self.assertIsNone(rows[0]["server"])
            self.assertEqual(rows[1]["server"], {"time_in_queue": 0.05})
            self.assertEqual((rows[1]["sent"], rows[1]["first"], rows[1]["done"]), (0.25, 0.45, 1.15))
            t = json.loads((out / "trajectories" / "s1.json").read_text())
            self.assertEqual(t["extra"]["input"], "inputs/llama31-8b-s1.jsonl")
            self.assertEqual(t["final_metrics"]["total_prompt_tokens"], 2001)


    def test_a_run_before_common_sh_names_its_input_by_seed(self):
        with tempfile.TemporaryDirectory() as tmp:
            d, out = Path(tmp) / "sync", Path(tmp) / "out"
            d.mkdir()
            (d / "probe_c1.json").write_text(json.dumps({
                "input_lens": [1001], "output_lens": [5], "ttfts": [0.1], "latencies": [0.5],
                "start_times": [100.0], "errors": [""], "itls": [[0.1] * 4]}))
            (d / "index.jsonl").write_text(json.dumps(
                {"name": "probe_c1", "seed": 1, "rate": "inf", "max_concurrency": "1"}) + "\n")
            self.assertEqual(bench_to_atif.convert(d, "probe_c1", "m", "qwen3-8b/sync/probe_c1", out), (1, 0))
            t = json.loads((out / "trajectories" / "s0.json").read_text())
            self.assertEqual(t["extra"]["input"], "inputs/qwen3-8b-s1.jsonl")


class Models(unittest.TestCase):
    def test_common_sh_and_convert_all_list_the_same_models(self):
        line = next(l for l in (HERE / "common.sh").read_text().splitlines() if l.startswith("MODELS=("))
        pairs = line.split("(", 1)[1].split(")", 1)[0].split()
        self.assertEqual(dict(reversed(p.rsplit(":", 1)) for p in pairs), convert_all.MODELS)


if __name__ == "__main__":
    unittest.main()
