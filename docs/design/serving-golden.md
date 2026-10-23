# The serving golden

Status: plan of record, #589. The A100 single-turn golden was collected on
2026-10-13; everything else here is planned, and each piece has a next step
below.

A program says how a deployment serves; a golden says whether it does. This
document decides what is measured on real engines to judge a program's
timing: on which deployment and workload, at which load, with which engine
release and settings, which client, and what one run records and where it
is kept.

## What a golden checks

The scheduler's choices are not a GPU question. Which request a step admits,
how many tokens it gives each, whom it preempts: the CPU oracle checks that,
by running vLLM's own scheduler from `ref/vllm` without a GPU, a step at a
time (`tools/vllm_oracle.py`, `tools/vllm_replay_oracle.py`,
`tools/oracle/*.out.*`). `scripts/check_oracle_gpu.py` holds the CPU oracle
and an A100 engine (`tools/oracle/a100_engine.json`, six scenarios) to the
same `first`, `done` and `preemptions`; the tests (`tests/vllm_oracle.rs`)
and the Lean model (`lean/Serq/Oracle.lean`) hold serQ to the oracle's
answers.

A golden checks **time, and the aggregates that move it**: per-request
time to first token, time per output token and response time; queueing
delay; step count and step time; preemptions; prefix-cache hits; KV
occupancy. It is measured on a released engine, unpatched and without
plugins, with what the engine itself exports and an existing client. Of
what the engine exports, the response `usage` and `/metrics` are there by
default; the cached tokens in `usage`, the OTLP spans, the iteration log,
the KV-cache metrics and the KV events each need a flag of the engine's
own, which [What one run records](#what-one-run-records) names. Collection
runs only on GPUs, on Lambda Cloud, and the records go to a private Harbor
Hub dataset, `servingq/serq-serving-traces`.

## What is collected, and what is not yet

Each collected batch is a task of the private Harbor dataset
`servingq/serq-serving-traces` ([Instances and storage](#instances-and-storage));
the table names the tasks.

| Collected | Where | What |
|---|---|---|
| A100 single-turn, vLLM v0.31.0, Llama-3.1-8B-Instruct and Qwen3-8B (2026-10-13) | Harbor tasks `servingq/a100-vllm031-single-turn-llama31-8b-2026-10` and `servingq/a100-vllm031-single-turn-qwen3-8b-2026-10`; scripts: #605, which adds them as `tools/a100-v031/` | `examples/workloads/single_turn.sq` against `examples/multi-turn/vllm.sq`, replayed by `vllm bench serve`. Per model: a saturation probe (1 to 96 concurrent requests; λ_max 3.72 req/s for Llama, 3.63 for Qwen), a Poisson sweep at 0.2, 0.4, 0.6, 0.8, 0.9, 1.0 and 1.2 × λ_max with three repetitions, the 1.0 point with async scheduling on, and seven engine settings at 0.9 × λ_max with three repetitions; 51 runs per model, all exit 0. Both models get the same (n, o) draws. Every request, 65 023 in all, is an ATIF-v1.7 trajectory that harbor 0.24.0's validator accepts |
| A6000 P/D pull and push (2026-09) | Harbor task `servingq/a6000-pd-nixl-2026-09`; part in `tools/a6000/` | pull 1P1D, push 1P1D and pull 2P2D at session spacing 3.0 s, 96 sessions of `examples/replay/data/short_base.csv` each, 288 ATIF trajectories. `/metrics` polled once a second, no step record; a toy round-robin proxy; vLLM at the dev commit `0c87a197` |

The single-turn records per run: the client's per-request JSON
(`--save-detailed`), the server's OTLP spans, its log with the iteration
lines, `/metrics` once a second and around each run, `nvidia-smi` every
100 ms, and the server flags. Not recorded there: KV events, the KV
residency histograms, CUDA graph padding, and a step-cost probe. In 18 runs
the spans of 1 to 5 requests are missing (the span receiver stopped before
the server's exporter sent them); the client records are complete. Such a
request keeps its trajectory and its row: its `extra.server` is null, its
`prompt` (and `metrics.prompt_tokens`) is the client's input length instead
of the span's `prompt_tokens`, and nothing is synthesised (#605's
`bench_to_atif.py`).
The step cost (`c_it` and the
rest of `examples/replay/vllm_replay.sq`'s worker) has not been refitted on
these records. The scripts that collected them land on their own, as they
ran (#605); the multi-turn scripts come with F2.

Not collected yet, each with its next step ([Next steps](#next-steps)):
multi-turn on the A100 (the continuity check, chat, a shared system prompt;
F2), the step-cost refit on synchronous data (F3), llm-d P/D on A6000 with
the real router (F4), and the P2 and P3 rows of the
[priority table](#deployment-and-workload-by-priority), which are not
scheduled. Nor is any program compared with a golden yet (F5).

## The goldens that exist

| Program | Golden | Limit |
|---|---|---|
| `examples/oracle/vllm_request.sq`, `examples/oracle/vllm_shared_prompt.sq` | the CPU oracle above | no time, by design: it checks the scheduler's choices, and that is enough for them |
| `examples/replay/vllm_replay.sq` (A100, Qwen3-8B) | `tools/a100/steps.jsonl` (3 024 steps, 3 022 of them fitted), `tools/a100/step_fit.json` | a step has no request id; the TTFT comparison is three points of one run each, and the run records are not published (`docs/use-cases/vllm.md`, sections *Measured replay* and *Reported comparison*); which CUDA graph mode the steps ran in is not recorded |
| `examples/multi-turn/vllm.sq` with `examples/workloads/single_turn.sq` | the two A100 single-turn tasks | the step cost is not refitted on them (F3) |
| `examples/pd-disaggregation/llmd_nixl_pull.sq`, `vllm_nixl_push.sq` | the A6000 task | polled `/metrics`, no step record; the program models llm-d's router, the run used a toy proxy; one load point; a dev commit, not a release. `docs/use-cases/pd.md` says the model has no hardware validation |
| `examples/engines/*.sq`, `vllm_lmcache.sq`, `thunderagent.sq` | none | source correspondence and interpreter tests only; `docs/use-cases/thunderagent.md` says the paper is not fitted |
| `examples/pd-disaggregation/pd_two_fabrics.sq` | none | its header says neither fabric's sharing policy is a measurement and asks for a run on two nodes, NVLink inside each and RDMA between them; no row below schedules one (the H100 row is the nearest) |

Out of scope: `examples/vendors/` (vllm-rbln and vllm-ascend are not GPUs);
the paper and theory programs (`examples/papers/*`, `mg1`, `ps`,
`closed`, `replica`, `routing`, `pd_ps`, `pd_tandem`, `pd_open`), which
closed forms and the Lean model check, and `fastertransformer.sq`, Dai et
al.'s model of FasterTransformer, whose claims are held to their paper;
`pd_batching.sq`, a what-if that runs `llmd_nixl_pull.sq`'s engines
colocated and split, whose engines F4 measures; and the language's own
examples (`arguments`, `separate_phases`), which have no deployment to
measure.

## Engine versions: releases

A golden names exactly which code ran (criterion 0), so each engine is a
final release, unpatched. The rule for each engine is the first final
release that contains the commit its example cites.

**vLLM v0.31.0** (released 2026-10-05, commit `db9527a4`). The `ref/vllm`
pin `0c87a197` is a development commit after v0.30.0, and v0.31.0 is 98
commits ahead of it and none behind (GitHub compare `0c87a197...v0.31.0`).
Those commits change the scheduler: the `skipped_waiting` queue is split
into `kv_holding_waiting` and `deferred_waiting` (vllm#58947), streaming
continuations get their own max tokens (vllm#57676), and the prefix-cache
key changes (vllm#51899, vllm#59335). Measured on 2026-10-23, serQ main
`4ed307c` against the v0.31.0 tag: of the 31 files `tools/citations.json`
cites, 15 differ between the two, and 122 of its 188 citations point at
different text at the same lines, all 85 in the scheduler. Moving the pin
is therefore its own change (F1), and it lands before any multi-turn
collection. Until then the CPU oracle runs the pin (vLLM 0.30.1rc0+215, as
`scripts/check_oracle_gpu.py` labels it), and that script holds it to A100
runs of v0.30.0: neither engine is v0.31.0, on which the single-turn golden
was collected before the pin moved. After F1 the oracle and the multi-turn
golden read the same engine. vLLM v0.32.0 (2026-10-18) exists;
the golden stays on v0.31.0 so that every collection shares one engine.

The other engines, checked against GitHub on 2026-10-23:

| Engine | Cited commit | Release | Note |
|---|---|---|---|
| SGLang | `b792228b` | v0.5.22 (2026-10-19) | the first release containing the commit; v0.5.21 predates it |
| TensorRT-LLM | `bf414e37` | none yet | no final release contains it; the latest final is v1.2.1 (2026-04-20), and the first tag that contains it is v1.4.0rc0 (2026-10-14), then v1.4.0rc1. P2 waits for v1.4.0, or runs v1.4.0rc1 recorded as a release candidate |
| TGI | `b4adbf2f` | v3.3.7 | the repository is archived; the commit is two after v3.3.7, which touch only a CodeQL workflow and docs, so the code is v3.3.7's |
| ThunderAgent | `7ddc861` | none | the repository has no releases; the commit itself |
| llm-d, its router | `13eebdb` (`llm-d/llm-d`), `8a2f37d` (`llm-d/llm-d-router`, formerly `llm-d-inference-scheduler`) | llm-d v1.0.0 (2026-10-20) | `docs/language.md` and `docs/use-cases/pd.md` name the two commits the other way round. v0.10.0 is the first llm-d release containing `13eebdb`, but its guides deploy router v0.11.0, which predates `8a2f37d`; v1.0.0's deploy router v1.0.0, the first final release containing it, and vLLM v0.31.0 (`guides/env.sh`, the `gpu-vllm` image) |

The model is Qwen3-8B in bf16 at TP 1 for every collection from here on.
The single-turn golden also has Llama-3.1-8B-Instruct.

## Engine settings: what serQ models

The golden measures the part of vLLM serQ models, not vLLM's default
deployment. On one GPU, v0.31.0 has one default that serQ does not model:
asynchronous scheduling, which is on unless something rules it out
([`vllm/config/vllm.py` L1576–1635](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/config/vllm.py#L1576-L1635)).
With it the engine schedules step n+1 before it has processed step n's
output, so slots, blocks and admissions lag a step, and a preempted request
waits for its in-flight output. `docs/language.md` §7 lists it as not
modelled. The rest follows the program's values; the API server's own
defaults on an A100 are a 2048-token budget and 256 requests
([`vllm/engine/arg_utils.py` L2868–2887](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/engine/arg_utils.py#L2868-L2887)).
For `examples/replay/vllm_replay.sq`:

```
vllm serve Qwen/Qwen3-8B --dtype bfloat16 --tensor-parallel-size 1 \
  --block-size 16 --max-num-batched-tokens 512 --max-num-seqs 64 \
  --num-gpu-blocks-override 8011 --enable-chunked-prefill --enable-prefix-caching \
  --no-async-scheduling --scheduling-policy fcfs --enable-prompt-tokens-details \
  --enable-logging-iteration-details --otlp-traces-endpoint <collector> \
  --kv-events-config <zmq publisher> --kv-cache-metrics --cudagraph-metrics
```

The single-turn golden ran `examples/multi-turn/vllm.sq`'s values instead:
a budget of 8192, `--max-num-seqs 64`, `--max-model-len 8192`, and the KV
capacity the GPU gave (163 008 tokens for Llama, 145 344 for Qwen, as
logged).

- **Asynchronous scheduling.** Off for the golden, and one point per
  workload, at the knee, measured with it on, so the difference is on
  record. In the single-turn golden at 1.0 × λ_max, turning it on took the
  mean TPOT from 56.8 to 46.3 ms for Llama and from 58.7 to 48.5 ms for
  Qwen (the same seed).
- **The CPU's time.** Without asynchronous scheduling the scheduler's CPU
  time is in series with the GPU's. `c_it` in `vllm_replay.sq` (4 ms) was
  calibrated with it on; F3 refits it on synchronous data.
- **CUDA graphs.** On, in vLLM's default mode (`FULL_AND_PIECEWISE`), for
  the golden and for the step-cost fit alike. With 64 sequences the capture
  sizes are 1, 2, 4, 8 and every 8 up to 128, twice `max_num_seqs`
  ([`vllm/config/vllm.py` L2453–2460](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/config/vllm.py#L2453-L2460)
  for the bound, [L2569–2576](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/config/vllm.py#L2569-L2576)
  for the list; the single-turn log lists the same 19), and a smaller step
  is padded to the next size. The manifest records the sizes from the log, and
  `--cudagraph-metrics` the padding. `tools/a100/step_fit.json` does not
  say its mode, so F3 does not mix it with the new data.
- **The null block.** vLLM takes one block of the pool as the null block
  ([`vllm/v1/core/block_pool.py` L185–188](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/v1/core/block_pool.py#L185-L188)),
  and its log's "GPU KV cache size" counts it
  ([`vllm/v1/core/kv_cache_utils.py` L2444–2471](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/v1/core/kv_cache_utils.py#L2444-L2471)):
  the single-turn run with `--num-gpu-blocks-override 2000` logged 32,000
  tokens. A program's `blocks` is `num_gpu_blocks` less the null block
  (`docs/use-cases/vllm.md`, *Settings*), so `vllm_replay.sq`'s 8010 blocks
  (128 160 tokens) run as `--num-gpu-blocks-override 8011`, whose log reads
  128,176 tokens; a run at the GPU's own capacity gives the program
  `--blocks` as the logged tokens over 16, less one. Whether the old A100
  testbed's 128 160 tokens counted the null block is not on record; the new
  golden does not depend on it.
- **The prefix cache.** vLLM shares blocks by their content; serQ shares
  what a hold's `prefix` keys say, and a session's chain is its own
  (`docs/language.md` §9; #553). A workload with a prefix common to several
  sessions, such as an agent's system prompt, is compared with a program
  that keys it, and the KV events show what vLLM actually shared. vLLM also
  caches a block as soon as it is scheduled, where serQ caches a prefix when
  its hold releases it (#575).
- **P/D.** The prefiller and the decoder both run `--no-async-scheduling`,
  and the KV transfer's `kv_load_failure_policy` is `fail`, v0.31.0's
  default ([`vllm/config/kv_transfer.py` L103–106](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/config/kv_transfer.py#L103-L106)),
  written out in the manifest.
- **What v0.31.0 restructures.** Prefill throttling is for data-parallel
  MoE and does not apply. `skipped_waiting`, the pin's queue of requests in
  a blocked status, is not renamed but split in two: `kv_holding_waiting`,
  a queue of the waiting requests that hold KV blocks, which a step drains
  before `waiting`, and `deferred_waiting`, the set of those skipped or
  blocked
  ([`vllm/v1/core/sched/scheduler.py` L212–219](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/v1/core/sched/scheduler.py#L212-L219),
  [L874–887](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/v1/core/sched/scheduler.py#L874-L887),
  [L2386–2404](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/v1/core/sched/scheduler.py#L2386-L2404)).
  A request now belongs by holding blocks, not by its status, so each serQ
  site that cites `skipped_waiting` is re-read against these lines in F1,
  not renamed.

## The client

Two steps, kept apart: **making a workload**, and **replaying it at a
chosen load while measuring**.

### Making a multi-turn workload with Harbor

A chat or agent workload is generated by `harbor run` (harbor v0.24.0) with
a simulated user over the ACP bridge (`--user-agent`, `--bridge acp`). The
bridge supports `claude-code`, `gemini-cli`, `codex` and `opencode` as
target agents, and any Harbor agent as the user
([`docs-mintlify/jobs/simulate-a-user.mdx` L58–67](https://github.com/harbor-framework/harbor/blob/v0.24.0/docs-mintlify/jobs/simulate-a-user.mdx?plain=1#L58-L67)).

- **The target agent is `opencode`.** Of the four, it is the one whose
  provider is a config that Harbor writes and a job overrides, so the job
  file alone sends its calls to our engine's OpenAI-compatible API: it
  supports the ACP bridge
  ([`src/harbor/agents/installed/opencode.py` L72–78](https://github.com/harbor-framework/harbor/blob/v0.24.0/src/harbor/agents/installed/opencode.py#L72-L78)),
  writes its provider block from `--model provider/model` and deep-merges a
  job's `opencode_config` over it
  ([L545–558](https://github.com/harbor-framework/harbor/blob/v0.24.0/src/harbor/agents/installed/opencode.py#L545-L558)),
  and does so in ACP mode too
  ([L178–186](https://github.com/harbor-framework/harbor/blob/v0.24.0/src/harbor/agents/installed/opencode.py#L178-L186)).
  The model is `vllm/Qwen/Qwen3-8B`, and the overlay names the engine:
  `{"provider": {"vllm": {"npm": "@ai-sdk/openai-compatible", "options":
  {"baseURL": "http://<engine>/v1"}}}}`. The other three targets default to
  their vendors' APIs.
- **Chat and agent.** Both run on terminal-bench tasks with Harbor's
  default persona; F2 picks the task set, as many sessions as a sweep's
  horizon needs, and records it. A chat session denies opencode's tools
  (its `permission` setting), so the user talks the task through with a
  target that cannot act, and a turn is the user's message and the model's
  answer. An agent or subagent session keeps the tools, and a tool's run
  time becomes the gap before the next call.
- **The simulated user is a hosted model.** F2 picks which and records it
  in the manifest. Its calls are not part of the trace; only the target's
  calls reach the engine under test. Its API cost is approved with F2's GPU
  session.
- F2's dry run checks, before any GPU is paid for, that each target turn is
  one call to the engine, and finds any call opencode makes on its own (a
  session title, a compaction) so that it is pointed away from the engine
  or marked in the trace.

Harbor writes an ATIF trajectory per session. An agent step is one LLM call;
the gap between steps is the user's time or a tool's.

### Replaying with aiperf

A workload is replayed by [aiperf](https://github.com/ai-dynamo/aiperf)
v0.13.0 (2026-09-24, its latest release), for every engine.

- **Its multi-turn replay means what a serQ trace means.** In fixed-schedule
  mode a session's first turn is sent at its timestamp and each later turn
  `delay` after the previous turn's response ends
  ([`src/aiperf/timing/strategies/fixed_schedule.py` L125–178](https://github.com/ai-dynamo/aiperf/blob/v0.13.0/src/aiperf/timing/strategies/fixed_schedule.py#L125-L178)),
  as a serQ trace starts session i at i × spacing and sends a turn `think`
  after the previous one completes. The load axis of a chat replay is
  therefore the session spacing, an open arrival of sessions.
- **It records every request.** `profile_export.jsonl` carries the
  conversation id, the turn index, `request_start_ns`, `request_ack_ns`,
  `request_end_ns`
  ([`src/aiperf/common/models/record_models.py` L173–237](https://github.com/ai-dynamo/aiperf/blob/v0.13.0/src/aiperf/common/models/record_models.py#L173-L237)),
  TTFT, the gap between chunks, input and output lengths, and the cached
  prompt tokens of `usage.prompt_tokens_details`
  ([`src/aiperf/metrics/types/usage_cache_metrics.py` L33–62](https://github.com/ai-dynamo/aiperf/blob/v0.13.0/src/aiperf/metrics/types/usage_cache_metrics.py#L33-L62)).
  It sends `X-Request-ID` with each request
  ([`src/aiperf/transports/base_transports.py` L125–126](https://github.com/ai-dynamo/aiperf/blob/v0.13.0/src/aiperf/transports/base_transports.py#L125-L126)),
  and vLLM takes that header as the base of the request's id
  ([`vllm/entrypoints/serve/engine/serving.py` L120–129](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/entrypoints/serve/engine/serving.py#L120-L129)).
  The chat endpoint prefixes it, `chatcmpl-<id>`
  ([`vllm/entrypoints/openai/chat_completion/serving.py` L291–293](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/entrypoints/openai/chat_completion/serving.py#L291-L293)),
  and the span carries that prefixed id
  ([`vllm/v1/engine/output_processor.py` L836](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/v1/engine/output_processor.py#L836)),
  so a client record and a server span join on the header once the prefix
  is stripped.
- **The two context modes.** aiperf's `ConversationContextMode`
  ([`src/aiperf/common/enums/enums.py` L166–193](https://github.com/ai-dynamo/aiperf/blob/v0.13.0/src/aiperf/common/enums/enums.py#L166-L193))
  says how a turn's prompt is built. A Harbor-generated session is replayed
  as `multi_turn` input in `DELTAS_WITHOUT_RESPONSES`, the default
  ([`src/aiperf/dataset/loader/base_loader.py` L92–99](https://github.com/ai-dynamo/aiperf/blob/v0.13.0/src/aiperf/dataset/loader/base_loader.py#L92-L99)):
  each turn is the text it adds, and aiperf threads the live answers into
  the history, so a session's prompt grows by its own answers, as serQ's
  chain does. The `short_base.csv` continuity check is replayed as
  `mooncake_trace` input with a `messages` array per turn, which aiperf sends
  as-is in `MESSAGE_ARRAY_WITH_RESPONSES`
  ([`src/aiperf/dataset/loader/mooncake_trace.py` L80–108](https://github.com/ai-dynamo/aiperf/blob/v0.13.0/src/aiperf/dataset/loader/mooncake_trace.py#L80-L108)),
  each turn's whole prompt as the old client sent it.
- **Two defaults written out.** `--cache-bust none` (the default; the other
  targets put a marker into the prompt, per session or during the warmup,
  and change what the prefix cache can share,
  [`docs/cli-options.md` L843–849](https://github.com/ai-dynamo/aiperf/blob/v0.13.0/docs/cli-options.md?plain=1#L843-L849)),
  and no `--inter-turn-delay-cap-seconds`, which would clamp a long think
  ([`src/aiperf/dataset/loader/multi_turn.py` L132–142](https://github.com/ai-dynamo/aiperf/blob/v0.13.0/src/aiperf/dataset/loader/multi_turn.py#L132-L142));
  the manifest checks that the loader clamped nothing.

### Which client for which workload

| Workload | Made by | Replayed by |
|---|---|---|
| single-turn Poisson | synthetic: `single_turn.sq`'s length distributions | `vllm bench serve` (collected) |
| shared system prompt (`vllm_system_prompt.sq`) | synthetic | aiperf: it is multi-turn |
| continuity check | `examples/replay/data/short_base.csv` | aiperf, `mooncake_trace` with `messages` |
| chat | `harbor run`, opencode with a simulated user, tools denied | aiperf, `multi_turn` |
| agent (`thunderagent.sq`), subagent (`vllm.sq` with `subagents.sq`) | `harbor run`, opencode with tools | aiperf, `multi_turn`; a tool's time is a `delay` |
| P/D (`llmd_nixl_pull.sq`, `vllm_nixl_push.sq`) | as chat | aiperf, through the real llm-d router |

### From a trajectory to a trace

The serQ trace has `examples/workloads/vllm_replay.sq`'s columns,
`session,turn,new,out,think,forced`, and `serq run --trace` reads it:

- `new` is the turn's whole prompt as the engine measured it,
  `usage.prompt_tokens`;
- `out` is `completion_tokens`;
- `think` is the next turn's sent time less this turn's done time;
- `forced` is 0.

The trace a run is compared against is built from **that run's** records,
so serQ sees the prompts the engine saw, chat-template tokens included. The
generation trajectory supplies the text and the think times aiperf replays:
a `multi_turn` line per session, the first turn's `timestamp` i × spacing
(ms), each later turn's `delay` its think (ms), `output_length` its
`completion_tokens`, and `ignore_eos` in the turn's `extra` request fields so
that the answer has that length
([`src/aiperf/dataset/loader/models.py` L97–119](https://github.com/ai-dynamo/aiperf/blob/v0.13.0/src/aiperf/dataset/loader/models.py#L97-L119)).

Checked on the A6000 pull 1P1D record, `tools/a6000/pull1x1_s3_rounds.jsonl`:
converted this way it gives 957 rows of 96 sessions whose `new`, `out` and
`forced` equal `short_base.csv`'s in every row, and whose `think` exceeds
the trace's by 0.5 to 31 ms (median 2 ms), the client's own delay. `serq run
examples/replay/vllm_replay.sq --workload examples/workloads/vllm_replay.sq
--trace <it> --N 96 --spacing 3.0 --horizon 3000` reads it and ends "arrivals
96 ended 96 turns 957". The converters themselves are F2's, where their
first real input is.

## Load

Each load point is a run of its own: the warmup is discarded, a measured
window is fixed, and the point is repeated three times with a different
seed or day. Each workload starts with a saturation probe, a closed loop
whose concurrency rises until the throughput is flat, which gives λ_max.

| Workload | Load axis | Points |
|---|---|---|
| single-turn (`vllm.sq` with `single_turn.sq`) | Poisson λ | 0.2, 0.4, 0.6, 0.8, 0.9, 1.0 and 1.2 × λ_max (collected) |
| continuity check (`vllm_replay.sq`, `llmd_nixl_pull.sq`; `short_base.csv`) | session spacing | the old measurements' points: A100 3.0 and 2.5 s, A6000 pull 1P1D 3.0 s |
| chat (a Harbor trace, `vllm_replay.sq`) | session spacing | the probe finds the collapse; six points close around it |
| shared system prompt (`vllm_system_prompt.sq`) | Poisson λ × system prompt S | λ as single-turn, S ∈ {0, 2k, 8k} tokens |
| subagent (`vllm.sq` with `subagents.sq`) | Poisson λ of agent sessions (`subagents.sq`'s `Lambda`), open | three points near λ_max |
| agent (`thunderagent.sq`) | concurrent agents N, closed | 1, 2, 4, 8, 16, 32, … until the KV is exceeded |
| P/D (`llmd_nixl_pull.sq`, `vllm_nixl_push.sq`) | chat spacing × NP:ND | the chat spacings × {1:1, 1:3, 3:1, 2:2} |

Apart from the load, each engine knob is moved one at a time at the knee:
`max_num_batched_tokens` {512, 2048, 8192}, `max_num_seqs`, prefix caching
on and off, chunked prefill on and off, and the KV capacity (lowered to
force preemption). The single-turn golden did seven: `B512`, `B2048`,
`seqs16`, `seqs128`, `noprefix`, `nochunk` and `kv2000` at 0.9 × λ_max.

Not every point can yet be put to its program. `vllm.sq` takes
`max_num_seqs` and the KV capacity as arguments (`--max_seqs`,
`--blocks`), so `seqs16`, `seqs128` and `kv2000` run as they are; its `B`
and the `Lambda` of `single_turn.sq` and `subagents.sq` are constants, so
`--Lambda 3.72` or `--B 512` is refused at link ("unknown program argument
`Lambda`"), and F5 makes them arguments first. Prefix caching off and
chunked prefill off are not arguments at all: they change `vllm.sq`'s
`reuse` and `taking kv` clauses, so each is a variant program, which F5
writes or leaves out of the comparison, and says which.

### The continuity check

The first multi-turn run of the new pipeline (v0.31.0, aiperf, the new
records) replays the trace and the points of the old measurements: the A100
at spacing 3.0 and 2.5 s (`docs/use-cases/vllm.md`, *Reported comparison*)
and the A6000 pull 1P1D at 3.0 s (`servingq/a6000-pd-nixl-2026-09`). The
old runs had asynchronous scheduling on (the A100's `c_it` was calibrated
with it, and `tools/a6000/serve_pd.sh` leaves the pin's default, which
turns it on), so the continuity runs keep it on, and the same points are
then repeated with it off. A disagreement is then the engine's or the
tools', and can be told apart; starting from a Harbor trace, or with
asynchronous scheduling off, would change two things at once.
`short_base.csv` serves only this check, not the sweeps: where it came
from is not written in the repository (`docs/use-cases/vllm.md` says
only "333 sessions").

Replacing it by a Harbor trace is a separate change after the check. It is
read today by `examples/workloads/vllm_replay.sq`,
`examples/workloads/short_base.sq` (the workload of `vllm_nixl_push.sq`),
`tests/outline.rs` (through `short_base.sq`), `pyserq/tests/test_pyserq.py`
and the example in `pyserq/src/lib.rs`, `scripts/lean_bench.py`,
`tools/vllm_replay_oracle.py`, `tools/a6000/pd_replay.py` and `run_mode.sh`,
and `docs/use-cases/vllm.md`.

## What one run records

A run is one load point × one repetition. Everything is switched on by a
flag of the released engine or client, and the columns follow serQ's
outputs so that F5's comparison is one diff: `serq run --dump DIR`
(`time,session,turn,value`) and `tools/oracle/cache_trace.out.csv`
(`session,turn,sent,first,done,prompt,cached,out`).

1. `manifest.json`: the instance type, region, time and price; the GPU,
   driver and CUDA; `nvidia-smi topo -m`; the NIC (IB or TCP, `UCX_TLS`);
   the engine release and its wheel's CUDA build; model, dtype and TP; every
   server and client flag; the KV tokens and CUDA graph sizes the log
   printed; the input trace, the load point, the repetition, the warmup and
   the measured window.
2. The client's per-request export, as the client writes it: aiperf's
   `profile_export.jsonl`, or `vllm bench serve --save-detailed` for
   single-turn. Per request: session, turn, request id, sent, first and
   done, the time of each chunk, prompt tokens, cached tokens (the
   response's `usage.prompt_tokens_details`, with
   `--enable-prompt-tokens-details`), output tokens, status.
3. `spans.jsonl`: the server's `llm_request` span per request, from
   `--otlp-traces-endpoint`: request id, TTFT, end-to-end, queue, prefill,
   decode and inference time, prompt and completion tokens
   ([`vllm/v1/engine/output_processor.py` L799–859](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/v1/engine/output_processor.py#L799-L859)).
   The gap between the client's `sent` and the span's start (the arrival)
   splits a request's fixed cost `c0` between the network, the API server
   and the engine. A span is lost when its receiver stops before the
   server's exporter sends it, as in 18 single-turn runs, so the scripts
   stop the server before the receiver, and the manifest counts the
   requests without a span.
4. `iterations.jsonl`: the step lines of `--enable-logging-iteration-details`
   ([`vllm/v1/metrics/loggers.py` L177–207](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/v1/metrics/loggers.py#L177-L207)):
   prefill requests and tokens, decode requests and tokens, elapsed time and
   KV usage, no request id. The elapsed time is the wait for the model's
   output after the step was scheduled and dispatched
   ([`vllm/v1/engine/core.py` L556–601](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/v1/engine/core.py#L556-L601),
   [L630–659](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/v1/engine/core.py#L630-L659)),
   so it leaves out scheduling and `update_from_output`. The step-cost fit
   uses it with the probe (9).
5. `metrics.jsonl`: `/metrics` once a second: running and waiting counts,
   `kv_cache_usage_perc`, prefix-cache queries and hits (local and
   external), preemptions, the per-request histograms
   (`request_queue_time_seconds`, `request_prefill_time_seconds`,
   `request_num_preemptions`, …), and with `--kv-cache-metrics` the KV
   blocks' lifetime, idle time before eviction and reuse gap. Those are
   sampled, 1 % of blocks by default
   ([`vllm/config/observability.py` L60–66](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/config/observability.py#L60-L66));
   the manifest records the rate. In P/D, the NIXL connector's transfer
   histograms and counters (`vllm:nixl_xfer_time_seconds`,
   `vllm:nixl_bytes_transferred`, `vllm:nixl_num_failed_transfers`, …,
   [`nixl/stats.py` L177–263](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/distributed/kv_transfer/kv_connector/v1/nixl/stats.py#L177-L263)),
   which are all a run has of its transfers.
6. `kv_events.jsonl`: `BlockStored`, `BlockRemoved` and `AllBlocksCleared`
   from `--kv-events-config`'s ZMQ publisher
   ([`vllm/distributed/kv_events.py` L56–140](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/distributed/kv_events.py#L56-L140)):
   what the prefix cache held and in which order it evicted, against the
   program's LRU and keyed segments (#553).
7. `router.jsonl` (llm-d, ThunderAgent): what the router logs by default,
   the instance each request was sent to and pause/resume. Scores per
   candidate are kept only if the router logs them.
8. `gpu.jsonl`: `nvidia-smi dmon` or DCGM every 100 ms: SM and memory
   bandwidth utilisation, power, clocks, temperature.
9. `probe.jsonl`: requests sent alone, without load, sweeping the prefill
   length, the decode batch and the KV length, for the step-cost fit. Once
   per instance.
10. `summary.json`, per load point: throughput, and TTFT, TPOT, ITL and
    response time (mean, p50, p90, p99), from the client's export (2),
    which has every request; waiting time from the spans (3), over the
    requests that have one, with their count; preemptions, prefix hit rate
    and KV usage from `/metrics` (5); the step count and mean step time
    from the iteration log (4). No goodput: the golden fixes no SLO, and a
    goodput at any SLO is recomputed from (2). Little's law (L = λW) is
    checked within the run, which screens the golden itself.
11. `trajectories.tar.gz`: an ATIF-v1.7 trajectory per session, made from
    (2) and (3). A Harbor-made workload keeps both the generation's
    trajectories and the replay's.

### The ATIF form

A trajectory is Harbor's interchange format
([RFC](https://github.com/harbor-framework/harbor/blob/v0.24.0/rfcs/0001-trajectory-format.md)).
Harbor's Pydantic models refuse undeclared fields, so every serving value
is under `extra`.

| ATIF | serQ golden |
|---|---|
| a trajectory | a session (a chat session, a single-turn request, an agent) |
| `session_id` | the run id (load point × repetition), shared by the run's trajectories |
| `trajectory_id` | `<run id>/s<session>` |
| `agent` | the engine: `name`, `version` (the release), `model_name`, `extra` (dtype, TP, the manifest's path) |
| user step | a turn's request: `message` (below), `extra` `turn`, `new_tokens`, `think_s` |
| agent step | a turn's response, `llm_call_count` 1: `message` the output, `metrics`, `extra` the times |
| `metrics` | `prompt_tokens`, `cached_tokens` (local and external, inside `prompt_tokens` as ATIF defines it), `completion_tokens`, `completion_token_ids`, `extra.cached_local` and `extra.cached_external` |
| agent step `extra.serq` | the columns of `cache_trace.out.csv`: `session, turn, sent, first, done, prompt, cached, out`, in seconds from `extra.t0` |
| agent step `extra` | `instance`, `server` (`arrival, queued, scheduled, first_token, last_token, num_preemptions`), `token_times`, `status` |
| `timestamp` | the user step's is the client's `sent`, the agent step's its `done` (ISO 8601, ms) |
| root `extra` | `run_id`, the program's path, the workload (trace, load axis, point, repetition), `t0`, the measured window |
| `final_metrics` | the session's token totals and `total_steps`; no `cost_usd` (an instance-hour is not an API price; the price is in the manifest) |

An agent's tool call is the agent step's `tool_calls`, its result the
`observation`, the tool's start and end in `observation.results[].extra`; a
pause, a resume and the router's choice in the step's `extra.route`. A
subagent is a trajectory in the root's `subagent_trajectories`, linked from
the parent's `observation.results[].subagent_trajectory_ref`. In P/D,
`extra.instance` is `{"prefill": "p0", "decode": "d1"}`; a transfer has no
record of its own to point to (see
[What is lost without a plugin](#what-is-lost-without-a-plugin)), only the
NIXL histograms of `metrics.jsonl`. Engine steps, KV events and GPU samples
have no place in ATIF and stay files of the run. The collection scripts keep only
trajectories that `harbor.utils.trajectory_validator` accepts. A request
whose span is lost keeps its trajectory, with `extra.server` null; its
`metrics` and `extra.serq` come from the client's export, and nothing is
synthesised.

**`message` holds the text the turn added.** A user step's `message` is
the user's message or the tool's observation, verbatim; token ids are not
kept. Sharing a prefix across sessions (#553) and a system prompt needs the
content, and the delta keeps a session's size linear in its length, where
`prompt_token_ids`, the whole history, would grow every turn. A synthetic
prompt stays in the task's `inputs/` and `message` is empty, as in the
single-turn golden.

## What is lost without a plugin

| Lost | Instead |
|---|---|
| which requests a step served with how many tokens, and whom it preempted | the CPU oracle checks the scheduler's choices; the golden counts and times steps |
| each request's preemption count | `request_num_preemptions`, a distribution |
| a step's start and its scheduling time | the iteration log's model wait and the probe's fit; if that is not enough, a plugin is reconsidered then |
| each P/D transfer's time | the NIXL metrics' sums and counts, and the lone-request probe |

## Deployment and workload by priority

| Priority | Deployment (program) | Workload | Checks | Status |
|---|---|---|---|---|
| P0 | vLLM, one GPU (`multi-turn/vllm.sq`) | single-turn | step cost, per-request TTFT, response and waiting time, preemptions | collected 2026-10-13; refit F3 |
| P0 | vLLM, one GPU (`replay/vllm_replay.sq`, `multi-turn/vllm_system_prompt.sq`) | continuity check, chat, shared system prompt | the same, plus prefix sharing across sessions (#553) and reading a prefix still being computed (#575). Every engine-form example shares this engine's lines, so it comes first | F2 |
| P1 | llm-d P/D pull (`llmd_nixl_pull.sq`) | chat | with the real llm-d router, not a toy proxy: lease hold time, transfer time and NIC sharing (`share.maxmin`), the decoder's `cached_tokens` | F4 |
| P2 | SGLang, TensorRT-LLM, TGI (`engines/*.sq`), P0's instance and model | single-turn, chat | engine neutrality: whether the programs reproduce the engines' differences under one workload (prefill first, `GUARANTEED_NO_EVICT`, block 1). Every engine's single-turn, vLLM's included, is replayed by aiperf on P2's instance, so that the engines differ and the client does not; the `vllm bench serve` single-turn golden is not P2's baseline | not scheduled; after F2 |
| P2 | vLLM with LMCache (`vllm_lmcache.sq`) | chat with a working set larger than the GPU's KV | L2 hit tokens, load time | not scheduled; after F2 |
| P3 | ThunderAgent (`thunderagent.sq`), J replicas | agent | pause and resume, the router's choice, the throughput ratio | not scheduled; on four A6000 replicas first |
| P3 | vLLM P/D push (`vllm_nixl_push.sq`) | chat | the collapsed operating point | not scheduled; on F4's instance |
| P3 | vLLM subagents (`vllm.sq` with `subagents.sq`) | subagent | fan-out waiting and prefix sharing | not scheduled; after F2 |

## Instances and storage

| Lambda instance | For | Why |
|---|---|---|
| `gpu_1x_a100_sxm4` | P0; P2's engines and LMCache | the device of `tools/a100/step_fit.json` and `vllm_replay.sq`, so the old fit carries over; the single-turn golden ran on it |
| `gpu_4x_a6000` | P1 and the P/D push (1P1D, 1P3D, 3P1D, 2P2D); P3's four-replica ThunderAgent | the 2026-09 device. Its IB dmabuf registration failed then and the transfers ran over TCP, so the manifest names the NIC |
| `gpu_8x_h100_sxm5` | ThunderAgent and P/D over NVLink or IB, at the paper's scale; two of them would give `pd_two_fabrics.sq` its two fabrics, NVLink inside a node and RDMA between | only if the four-replica A6000 run shows the regime cannot be reached at that scale |

Every launch is a paid action: the user approves the instance type and its
$/h, quoted from Lambda when asked, before it starts.

The records go to Harbor Hub `servingq/serq-serving-traces` (private), one
task per measured batch. A task's raw records are under
`environment/data/<run id>/`, its trajectories in one `trajectories.tar.gz`,
and its instruction and test ask for a summary that the data recomputes, as
`servingq/a6000-pd-nixl-2026-09` does. `harbor sync`, then `harbor publish
<dataset dir> --private`.

## What the single-turn collection taught

- Lambda's `gpu_1x_a100_sxm4` came with driver 570 (CUDA 12.8). The vLLM
  0.31.0 wheel on PyPI is built for CUDA 13.0 and sees no GPU there; the
  release's `+cu129` wheel, installed with `--torch-backend=cu129`, does,
  and setup checks `torch.cuda.is_available()`.
- A prompt of random tokens does not re-encode to its own length, and a loop
  that matched lengths hung for two hours on a paid GPU. Prompts are now
  vocabulary words that round-trip, retries are capped, and every step
  before the main run has a time limit.
- `harbor publish` fails with a "statement timeout" on a task of about
  33 000 files (about 320 are fine), so the trajectories ship as one archive.
- Launching and terminating an instance costs money: the type and $/h are
  confirmed first.

## Next steps

Each collection is a paid GPU session that the user approves, instance type
and $/h first, so none of them is part of this document's change. The
pin move and the comparison need no GPU. Each step is an issue of its own.

**F1. Move `ref/vllm` to v0.31.0 (#760).** Before any multi-turn
collection, so that the CPU oracle and the golden read one engine.

- `scripts/fetch_vllm_ref.sh`: `REV` from `0c87a197…` to
  `db9527a46873454610df6dbedf79a36d6bf1a7f6`.
- Re-read and re-point every citation that says something else at the
  same lines (122 of 188 at `4ed307c`, all 85 in the scheduler), then
  `make citations`.
- Re-read each site that cites `skipped_waiting` against v0.31.0's split
  (*What v0.31.0 restructures* above) and fix it there: `docs/language.md`
  (§3, *Pools*, and the chunked-prefill row of §7's table),
  `docs/use-cases/pd.md` (*A decode instance*),
  `examples/pd-disaggregation/vllm_nixl_push.sq`, `tests/pd_semantics.rs`
  and `tools/serq_vllm.py`.
- The `RELEASED` entry for the LMCache connector in
  `scripts/check_citations.py` becomes redundant; the `v0.31.0rc2` links of
  `docs/use-cases/vllm.md` and `docs/use-cases/index.md` move to the
  release.
- Regenerating the oracles needs torch, because `tools/vllm_oracle.py`
  imports vLLM from `ref/vllm` (a CPU build is enough); the label of
  `scripts/check_oracle_gpu.py` moves with it. It overlaps #459, which also
  touches `tools/vllm_oracle.py`.
- After it, the vLLM links in this document can become `name.py:N`
  citations that `scripts/check_citations.py` checks.

**F2. P0 multi-turn on the A100 (aiperf; #761).** After F1. Needs a paid
`gpu_1x_a100_sxm4` session, and the simulated user's API budget, approved
first.

- First, without a GPU: aiperf v0.13.0 against a mock OpenAI server, to
  check the fixed schedule, `delay`, both context modes and that nothing is
  clamped; and `harbor run` with opencode against the mock, to check that a
  turn is one call.
- The two converters (trajectory to aiperf input, run records to the serQ
  trace), with a test that turns `tools/a6000/pull1x1_s3_rounds.jsonl` into
  `short_base.csv`'s rows.
- The choices left to it: the simulated user's hosted model and the
  terminal-bench task set, both recorded in the manifest.
- On the GPU: the continuity runs (3.0 and 2.5 s, asynchronous scheduling
  on as in the old runs, then off), then the chat and
  shared-system-prompt sweeps, with `probe.jsonl`, the iteration log, KV
  events and the KV and CUDA graph metrics; published as Harbor tasks.

**F3. Refit the step cost on synchronous data (#762).** `c_it` and the worker of
`examples/replay/vllm_replay.sq`, from F2's probe and iteration log. Needs
F2's session to have run the probe, or a paid A100 session for it; the fit
itself runs on a CPU. It moves numbers, so the sim-reviewer runs; it
regenerates `tools/oracle/*.ir.json` and updates `docs/use-cases/vllm.md`.
The single-turn synchronous records are on Harbor for a first look.

**F4. P1, llm-d P/D on A6000 with the real router (#763).** Needs a paid
`gpu_4x_a6000` session, approved first, and F2's converters. It runs llm-d
v1.0.0 and the router v1.0.0 its guides deploy (the engines table above).
On the GPU: the NIC (IB or TCP) checked and recorded, then
the continuity run (pull 1P1D at 3.0 s, against
`servingq/a6000-pd-nixl-2026-09`, asynchronous scheduling on, then off),
then the chat spacings × {1:1, 1:3, 3:1, 2:2} through the llm-d router.

**F5. Compare a program with its golden (#764).** No GPU. The
counterpart, for time, of `scripts/check_oracle_gpu.py`, which holds the
CPU oracle and the A100 engine to the same choices.

- First the knobs a sweep moves become program arguments (`args.number`,
  their defaults kept): `Lambda` in `examples/workloads/single_turn.sq` and
  `subagents.sq`, and `B` in `examples/multi-turn/vllm.sq`, inside its
  `fn main()`, where a program's arguments are declared. Prefix caching
  off and chunked prefill off are variant programs, written or left out,
  and the script says which.
- `scripts/check_golden.py`: for each collected point it runs the program
  with that point's arguments (`serq run … --dump`) and sets its mean and
  p90 TTFT, TPOT and response time, its preemptions and its prefix hit
  rate beside the golden's three repetitions (`summary.json` and the
  trajectories' `extra.serq` rows).
- The tolerance is decided after F3, from the spread of the single-turn
  repetitions, and written into the script and this document. Until then
  the script reports how far a program is and judges nothing.

P2 and P3 stay rows of the [priority table](#deployment-and-workload-by-priority)
until F2 and F4 land.

## Self-critique

**A plugin for per-step request ids.** An engine plugin could log which
request each step served and whom it preempted. Rejected: the golden would
measure a patched engine, and the scheduler's choices are the CPU oracle's
question, which answers them without a GPU.

**`vllm bench serve` or vLLM's `benchmarks/multi_turn` for multi-turn.**
`vllm bench serve`'s timed trace is an open loop of fixed timestamps
([`vllm/benchmarks/datasets/datasets.py` L1425–1452](https://github.com/vllm-project/vllm/blob/v0.31.0/vllm/benchmarks/datasets/datasets.py#L1425-L1452)):
it cannot send a turn after the previous one ends. `benchmarks/multi_turn`
waits a Poisson interval between requests rather than the trace's think
([`benchmark_serving_multi_turn.py` L801–803](https://github.com/vllm-project/vllm/blob/v0.31.0/benchmarks/multi_turn/benchmark_serving_multi_turn.py#L801-L803))
and estimates the cached share from the history's length
([L484–487](https://github.com/vllm-project/vllm/blob/v0.31.0/benchmarks/multi_turn/benchmark_serving_multi_turn.py#L484-L487)).
Rejected for aiperf, which is also the same client for every engine.
vLLM's single-turn golden stays on `vllm bench serve`, which was collected
and needs no session order; P2, which compares engines, replays
single-turn with aiperf for all of them.

**Harbor as the replayer.** Harbor has no timed resend: `--load-trajectory`
loads a past trajectory as the agent's context to continue the session, and
cannot be combined with a simulated user
([`docs-mintlify/jobs/loading-trajectories.mdx`](https://github.com/harbor-framework/harbor/blob/v0.24.0/docs-mintlify/jobs/loading-trajectories.mdx?plain=1#L1-L36)).
Rejected: Harbor makes the workload, aiperf replays it.

**Another target agent.** `claude-code`, `gemini-cli` or `codex` might be
pointed at vLLM through their own environment settings. Not tried, and
rejected for now: with opencode the route to the engine is in the job file,
through the adapter Harbor ships, and one target keeps the generated
workloads comparable.

**Asynchronous scheduling as the golden.** It is what a default deployment
runs, and the knee point records it. Rejected as the golden because serQ
does not model it, and a golden of what the program cannot express would
measure the language's gap, not the program.

**`short_base.csv` for the sweeps.** Its provenance is not written down.
Rejected beyond the continuity check, which needs it precisely because the
old measurements used it.

**`prompt_token_ids` in ATIF.** Exact, but the whole history in every turn,
so a session's size grows with the square of its length. Rejected for the
turn's text.

**H100 first.** Closer to ThunderAgent's paper, and dearer. Rejected until
the four-replica A6000 run shows the regime cannot be reached on it.

**#605 folded into the multi-turn scripts.** One directory for all
collection scripts. Rejected: the single-turn scripts are where the
published data came from and are finished; the multi-turn client is
untested without a GPU, and waiting would tie finished work to unstarted
work.

**The latest vLLM.** v0.32.0 came out on 2026-10-18. Rejected for now: the
single-turn golden and F1's pin are v0.31.0, and a golden whose collections
span two engines cannot tell an engine change from a program's error. A
later move is its own change, with the collections redone.

**A simulated user's think time.** The gap a generated chat records is the
hosted user model's latency, not a person's. It is kept as recorded: the
sweep's axis is the session spacing, and a think distribution of our own
would be a second workload decision hidden in the converter.

**Weak: it measures serQ's configuration, not the default deployment.**
Asynchronous scheduling off, the program's budget and slots, a fixed KV
size: a golden that passes says the program models this configuration. The
knee point with asynchronous scheduling on says how far the default is.

**Weak: the iteration log has no scheduling time.** Its elapsed time is the
wait for the model, so the CPU's share of a step is inferred from the probe
and the end-to-end step count. If that is not enough, the plugin question
is reopened.

**Weak: a golden has no pass mark yet.** F5 sets a program beside its
golden and reports the distance; what distance passes waits for F3's
refit and the spread of the repetitions, so until then a golden says how
far a program is, not whether it is right.

**Weak: the data is private.** The Harbor dataset is private, so a reader
of this repository cannot rerun a comparison without access. The scripts,
the summaries a task's test recomputes, and the numbers the use cases quote
are public.

**Weak: the upstream links here are not checked.** `scripts/check_citations.py`
checks `name.py:N` citations at the pin, `0c87a197`. The vLLM v0.31.0,
aiperf v0.13.0 and harbor v0.24.0 code here is cited as links to their tags,
which nothing checks; the vLLM ones can become checked citations after F1.
