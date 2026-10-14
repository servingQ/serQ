# 2026-10 A100 single-turn — vLLM v0.31.0, {short} 측정 원본

수집일 2026-10-13. serQ `examples/single-turn/vllm_single_turn.sq`의 workload를 실제 vLLM v0.31.0(정식 릴리즈)에 `vllm bench serve`로 보내고 받은 기록이다 (servingQ/serQ#589). 같은 노드, 같은 입력 길이로 Llama-3.1-8B-Instruct와 Qwen3-8B를 각각 쟀고, 이 task는 {short}이다.

## 셋업

| 항목 | 값 |
| --- | --- |
| 노드 | Lambda `gpu_1x_a100_sxm4` (us-east-1): A100-SXM4-40GB × 1, 드라이버 570.148.08 (CUDA 12.8) |
| vLLM | 0.31.0 정식 릴리즈, CUDA 12.9 wheel (`vllm-0.31.0+cu129`, 기본 cu130 wheel은 이 드라이버에서 GPU를 못 본다), torch 2.13.0+cu129 |
| 모델 | `{model}`, bf16, TP 1, `max_model_len 8192` |
| 엔진 (기본) | 블록 16, `max_num_batched_tokens 8192`(프로그램의 `B`), `max_num_seqs 64`, chunked prefill, prefix caching, FCFS, `--no-async-scheduling`(serQ는 async를 모델링하지 않는다), `gpu_memory_utilization` 기본 → KV {kv} 토큰 |
| 기록 | `--enable-prompt-tokens-details`, `--enable-logging-iteration-details`(step마다 로그 한 줄), `--otlp-traces-endpoint`(요청마다 `llm_request` span, `scripts/otlp_sink.py`가 받음) |
| workload | 요청마다 prompt n = ceil(uniform(1000, 3000)) 토큰, output o = floor(exp(평균 200)) + 1 토큰 (`ignore_eos`, `max_tokens = o`), Poisson 도착. prompt는 모델 어휘에서 고른 단어 토큰으로 정확히 n 토큰 (`scripts/gen_dataset.py`, seed 1·2·3 → `inputs/`). 길이 draw는 seed에만 의존하므로 두 모델이 같은 (n, o)를 받는다 |
| 클라이언트 | `vllm bench serve --backend vllm --dataset-name custom --custom-output-len -1 --skip-chat-template --ignore-eos --save-detailed`, 요청 id `<tag>/<dir>/<name>/<i>` |

## 실행 (`runs/`)

| 디렉터리·파일 | 내용 |
| --- | --- |
| `sync/probe_c<C>.json` | 포화 probe: 동시 요청 C ∈ {{1,2,4,8,16,32,64,96}}, closed loop. λ_max = 최대 `request_throughput` = {lmax} req/s (`lambda_max.txt`) |
| `sync/sweep_f<f>_r<rep>.json` | Poisson sweep: rate = f × λ_max, f ∈ {{0.2, 0.4, 0.6, 0.8, 0.9, 1.0, 1.2}}, 반복 rep = 1·2·3 (데이터셋 seed와 도착 seed가 rep). 점마다 도착 240 s분(최소 200 요청) |
| `async/sweep_f1.0_r1.json` | 같은 1.0 × λ_max 점을 async scheduling을 켜고 (기본 배포와의 차이) |
| `knob_<설정>/knee_r<rep>.json` | 0.9 × λ_max에서 설정 하나만 바꿔 3회: `B512`, `B2048` (`max_num_batched_tokens`), `seqs16`, `seqs128` (`max_num_seqs`), `noprefix`, `nochunk`, `kv2000` (`--num-gpu-blocks-override 2000`, 32 000 토큰, 선점을 강제) |
| 서버 디렉터리마다 | `flags.txt`(서버 플래그), `server.log`(iteration 로그 포함), `spans.jsonl`(요청별 span), `metrics.jsonl`(`/metrics` 1 s), `gpu.csv`(`nvidia-smi` 100 ms), `index.jsonl`(run마다 시작·끝 시각, rc), `<name>.metrics_before/after.txt`(run 전후 `/metrics` 전체), `<name>.log`(bench 출력) |
| `driver.log` | 수집 순서와 rc. 모든 run rc=0 |

## 파일 형식

- `<name>.json` — `vllm bench serve --save-detailed` 결과. 요청 i마다 `input_lens`, `output_lens`, `ttfts`, `itls`, `latencies`, `start_times`, `errors` (같은 순서). 요약 지표는 `mean/median/p99_{{ttft,tpot,itl,e2el}}_ms`, `request_throughput`.
- `spans.jsonl` — span 한 줄. `llm_request`의 `attrs`: `gen_ai.request.id`(`cmpl-<요청 id>-0`), `gen_ai.latency.{{time_to_first_token, e2e, time_in_queue, time_in_model_prefill, time_in_model_decode, time_in_model_inference}}`(초), `gen_ai.usage.{{prompt_tokens, completion_tokens}}`. 266 run 중 18 run에서 요청 1–5개의 span이 없다 (대부분 서버를 내리기 직전 run, exporter가 내보내기 전에 수신기를 껐다). 클라이언트 기록은 빠짐없다.
- `server.log`의 `Iteration(k): a context requests, b context tokens, c generation requests, d generation tokens, iteration elapsed time: t ms, GPU KV cache usage: u%` — step마다. 경과 시간은 schedule 뒤 모델 출력을 기다린 시간이다 (vLLM `core.py:640-649`). 로그 시각은 초 단위라 step 시작 시각은 없다.
- `trajectories.tar.gz` 안의 `<dir>/<name>/trajectories/s<i>.json` — 요청 i를 ATIF-v1.7 trajectory로 (user step + agent step). `extra.serq`는 serQ `cache_trace` 열 (`session, turn, sent, first, done, prompt, cached, out`, 첫 요청 기준 초), `extra.server`는 span의 시간. 같은 디렉터리의 `requests.jsonl`이 그 열만 모은 것. harbor 0.24.0 `trajectory_validator` 통과 (두 모델 합 65 023개).
- `inputs/<tag>-s<seed>.jsonl` — bench 입력 (`prompt`, `output_tokens`, 목표 길이 `n`, 실제 토큰 수 `tokens`).
- `scripts/` — 셋업, 데이터셋 생성, 수집 (`run_all.sh` → `run_model.sh`, `run_knobs_all.sh` → `run_knobs.sh`), span 수신기, 지표 수집, ATIF 변환.

## 주의

- golden은 "serQ가 모델링하는 부분"의 측정이다: async scheduling을 껐다. async를 켜면 같은 점에서 TPOT가 낮다 (`async/`와 `sync/sweep_f1.0_r1` 비교).
- CUDA graph는 기본(켜짐)이다. step 비용 fit은 같은 모드로 해야 한다.
- 입력 prompt는 무작위 단어라 요청 사이 prefix 공유가 없다 (`noprefix`와 기본이 같은 이유).

## 재현

`scripts/setup.sh` (HF_TOKEN 필요, Llama는 gated) → `scripts/run_all.sh` → `scripts/run_knobs_all.sh`. 이 드라이버에서는 setup 뒤 `vllm-0.31.0+cu129` wheel을 `--torch-backend=cu129`로 다시 깔았다. 변환: `scripts/convert_all.py RESULTS OUT`.
