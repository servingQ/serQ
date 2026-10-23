# A100 single-turn golden, vLLM v0.31.0 (#589)

The scripts that collected the single-turn serving records on one
A100-SXM4-40GB (Lambda `gpu_1x_a100_sxm4`, 2026-10-13): vLLM v0.31.0, the
release, serving Llama-3.1-8B-Instruct and Qwen3-8B, replaying with
`vllm bench serve` the workload `examples/workloads/single_turn.sq` run against
`examples/multi-turn/vllm.sq` (`examples/single-turn/vllm_single_turn.sq` at
collection time; the distributions are unchanged). The records
are not in this repository; they are in Harbor Hub `servingq/serq-serving-traces`
(private), one task per model:
`servingq/a100-vllm031-single-turn-{llama31-8b,qwen3-8b}-2026-10`, with a
Korean README of every file (`README_data.md` is its template).

Per model: a saturation probe (1-96 concurrent requests), a Poisson sweep at
0.2-1.2 x lambda_max with three repetitions, the 1.0 x point with async
scheduling on, and seven engine settings one at a time at 0.9 x lambda_max.
The engine runs with `--no-async-scheduling` and the program's token budget;
#589, section "엔진 설정: serQ가 모델링하는 부분에 맞춘다", gives the reasons.
In serQ, one sweep run (the base settings) is

```
serq run examples/multi-turn/vllm.sq --workload examples/workloads/single_turn.sq \
  --max_seqs 64 --blocks 10187 --arrivals <num_prompts> --horizon 3600
```

with `Lambda` in `single_turn.sq` edited to the run's `rate` (a plain `let`,
so not an option) and `num_prompts` from `index.jsonl`, and `--blocks 9083`
for Qwen3-8B: vLLM logged 163 008 and 145 344 KV tokens, 10 188 and 9 084
blocks of 16, one of which is the null block
(`vllm/v1/core/block_pool.py:187`, v0.31.0) that `blocks` leaves out.

| Script | Runs on | Does |
| --- | --- | --- |
| `setup.sh` | GPU node | vLLM v0.31.0 (CUDA 12.9 wheel), both models; needs `HF_TOKEN` |
| `gen_dataset.py` | GPU node | the workload as a `vllm bench` custom dataset, exact prompt lengths |
| `common.sh` | GPU node | the models, engine flags, server start and stop, the prompts, one bench run |
| `run_all.sh` → `run_model.sh` | GPU node | probe, sweep, async point |
| `run_knobs_all.sh` → `run_knobs.sh` | GPU node | the engine settings |
| `otlp_sink.py`, `metrics.py` | GPU node | per-request spans, `/metrics` once a second |
| `rsh.py` | local | ssh/sftp through paramiko, when the local OpenSSH cannot run |
| `convert_all.py` → `bench_to_atif.py` | local | every request as an ATIF-v1.7 trajectory and serQ `cache_trace` columns |
| `build_tasks.py` | local | the Harbor tasks; `harbor publish` then uploads them |
| `summarize.py` | local | one line per bench result |
| `test_tools.py` | `make check` | `metrics.py`'s keys and `bench_to_atif.py`'s join on made-up input; one model list |

Two things learnt the hard way: a prompt of random tokens does not re-encode
to its own length, so `gen_dataset.py` builds prompts from words that do; and
the Harbor registry times out on a task of tens of thousands of files, so the
trajectories ship as one `trajectories.tar.gz`.

## The records as collected

Each task's `scripts/` holds the scripts as they ran on 2026-10-13, and
`README_data.md` and the strings in `build_tasks.py` are the published text.
The scripts here have been fixed since. What the published records carry:

- **Prefix-cache hits.** Every warm-up and every probe sent the first prompts
  of seed 1's shuffle, and so did the first run after each warm-up, on the
  warm-up's server. Of their prompt tokens (`vllm:prefix_cache_hits_total` over
  `vllm:prefix_cache_queries_total`, `<name>.metrics_after.txt` less
  `metrics_before.txt`) hit: probe c=1 53.5%, c=2 99.6%, c=4 99.6%,
  c=8 63.4%, c=16 48.8%, c=32 3.2% (Llama) and 2.4% (Qwen), c=64 0.2%,
  c=96 0; `knee_r1` of every setting but `noprefix`, and the async point,
  1.7-2.1% (the warm-up's 16 prompts); every sync sweep run 0. lambda_max is
  probe c=96's throughput, so the sweep is not affected; the low-concurrency
  probes are. `README_data.md`'s "요청 사이 prefix 공유가 없다" holds for
  the sweep only. The scripts now give the warm-up and each probe their
  own slice of seed 0's prompts.
- **`metrics.jsonl` keys.** A series with a `reason`, `source` or
  `sleep_state` label after `model_name` lost it, and the last series printed
  overwrote the others: `num_requests_waiting_by_reason{engine="0"}` is
  `deferred`, `prompt_tokens_by_source_total{engine="0"}` is
  `external_kv_transfer` (always 0), `engine_sleep_state` is `discard_all`.
  The before and after snapshots hold every series. `metrics.py` now keys a
  series by every label but `model_name`, `engine` included, and leaves out
  the `_info` gauges, which the snapshots hold.
- **Spans.** Of the 120 runs (102 measured, 18 warm-ups), 18 measured runs
  miss the spans of 1-5 requests each; `README_data.md`'s "266 run 중 18 run"
  has the wrong total. 15 of the 18 are the last run on their server, whose
  sink was stopped together with vLLM while a batch could still be in the
  exporter; the other 3 are not explained. `stop_server`
  now waits out the exporter's 5 s interval, stops vLLM, which flushes its
  spans as it exits, and only then the sink.
- **Request ids.** `vllm bench serve` gives request i the id
  `<prefix><i>`, i its place in the input shuffled with the run's seed
  (`vllm/benchmarks/datasets/datasets.py:2540-2542, 2612`, v0.31.0). The
  prefix was `<tag>/<name>/` in `sync/` and `async/` (so the two
  `sweep_f1.0_r1` share ids) and `<tag>/<dir>/<name>/` in `knob_*/`;
  `README_data.md` writes the second for all. It is now the second everywhere.
- **Prompt lengths.** `gen_dataset.py` makes a prompt of n tokens without
  special tokens; the bench and the engine count them with, and Llama-3.1's
  tokenizer adds BOS: `input_lens` and the span's `prompt_tokens` are n + 1
  for Llama, n for Qwen3 (`datasets.py:2606`, v0.31.0).
- **Trajectory notes.** Every trajectory's `notes` says the prompts are
  random tokens in `data/<model>-s<seed>.jsonl`. They are words of the model's
  vocabulary, in `inputs/<tag>-s<seed>.jsonl`; `bench_to_atif.py` now says so
  and names the file in `extra.input`.
- `README_data.md`'s `core.py:640-649` is `vllm/v1/engine/core.py` at v0.31.0.

The scripts also changed in ways the records do not show: a failed run now
stops `run_model.sh` (and its setting, in `run_knobs.sh`) and the drivers exit
1 without their `DONE` line; every bench run has the 1800 s limit only the
settings had; `convert_all.py` leaves out a run whose exit code is not 0.
