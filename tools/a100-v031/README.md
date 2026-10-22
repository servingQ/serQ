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
#589 "Engine settings" gives the reasons.

| Script | Runs on | Does |
| --- | --- | --- |
| `setup.sh` | GPU node | vLLM v0.31.0 (CUDA 12.9 wheel), both models; needs `HF_TOKEN` |
| `gen_dataset.py` | GPU node | the workload as a `vllm bench` custom dataset, exact prompt lengths |
| `run_all.sh` → `run_model.sh` | GPU node | probe, sweep, async point |
| `run_knobs_all.sh` → `run_knobs.sh` | GPU node | the engine settings |
| `otlp_sink.py`, `metrics.py` | GPU node | per-request spans, `/metrics` once a second |
| `rsh.py` | local | ssh/sftp through paramiko, when the local OpenSSH cannot run |
| `convert_all.py` → `bench_to_atif.py` | local | every request as an ATIF-v1.7 trajectory and serQ `cache_trace` columns |
| `rounds_to_atif.py` | local | the same for `tools/a6000/*_rounds.jsonl` (multi-turn) |
| `build_tasks.py` | local | the Harbor tasks; `harbor publish` then uploads them |
| `summarize.py` | local | one line per bench result |

Two things learnt the hard way: a prompt of random tokens does not re-encode
to its own length, so `gen_dataset.py` builds prompts from words that do; and
the Harbor registry times out on a task of tens of thousands of files, so the
trajectories ship as one `trajectories.tar.gz`.
