# Sourced by run_all.sh, run_model.sh, run_knobs.sh and run_knobs_all.sh (from
# ~/g, venv active; the functions need M, TAG and R): the models, the engine's
# flags, its start and stop, the prompts, and one bench run.
export OTEL_EXPORTER_OTLP_TRACES_PROTOCOL=http/protobuf HF_HUB_OFFLINE=1
MODELS=(meta-llama/Llama-3.1-8B-Instruct:llama31-8b Qwen/Qwen3-8B:qwen3-8b)   # model:tag
PORT=8000
DUR=240                       # seconds of arrivals per sweep point
FLAGS=(--dtype bfloat16 --tensor-parallel-size 1 --max-model-len 8192
  --block-size 16 --max-num-batched-tokens 8192 --max-num-seqs 64
  --enable-chunked-prefill --enable-prefix-caching --scheduling-policy fcfs
  --enable-prompt-tokens-details --enable-logging-iteration-details
  --otlp-traces-endpoint http://127.0.0.1:4318/v1/traces)

log() { echo "$(date -u +%FT%TZ) $*" | tee -a "$R/driver.log"; }

# The prompts. Seeds 1-3 are the measured runs'; everything else takes a slice
# of seed 0's, no two the same: a prompt the server has already seen is a
# prefix-cache hit (README.md, "The records as collected").
inputs() {
  mkdir -p data
  for s in 0 1 2 3; do
    [ -f "data/$TAG-s$s.jsonl" ] && continue
    python gen_dataset.py "$M" 3000 "$s" "data/$TAG-s$s.jsonl" >> "$R/driver.log" 2>&1 || return 1
  done
  slice warmup 0 16
}
slice() {   # $1 name, $2 first line (from 0), $3 lines: data/$TAG-$1.jsonl, from seed 0
  sed -n "$(( $2 + 1 )),$(( $2 + $3 ))p" "data/$TAG-s0.jsonl" > "data/$TAG-$1.jsonl"
  [ "$(wc -l < "data/$TAG-$1.jsonl")" -eq "$3" ] || { log "seed 0 has too few prompts for $1"; return 1; }
}

PIDS=()
start_server() {   # $1 server dir, then the engine's flags
  local d=$1; shift; mkdir -p "$d"
  printf '%s\n' "$@" > "$d/flags.txt"
  python otlp_sink.py 4318 "$d/spans.jsonl" & PIDS+=($!)
  nohup vllm serve "$M" --port $PORT "$@" > "$d/server.log" 2>&1 & PIDS+=($!)
  for _ in $(seq 600); do curl -sf localhost:$PORT/health >/dev/null && break; sleep 2; done
  curl -sf localhost:$PORT/health >/dev/null || { log "server did not start: $d"; stop_server; return 1; }
  python metrics.py $PORT "$d/metrics.jsonl" & PIDS+=($!)
  nvidia-smi --query-gpu=timestamp,utilization.gpu,utilization.memory,power.draw,clocks.sm,clocks.mem,temperature.gpu \
    --format=csv,nounits -lms 100 > "$d/gpu.csv" & PIDS+=($!)
  log "server up: $d"
  # warm-up, discarded
  bench "$d" warmup "data/$TAG-warmup.jsonl" 0 inf 4 16 >/dev/null 2>&1 || true
}
stop_server() {
  # vLLM exports its spans through a BatchSpanProcessor, every 5 s by default,
  # and flushes it at exit (vllm/tracing/otel.py:85-88, v0.31.0): let the last
  # batch go, stop vLLM (PIDS[1]), wait for it to exit, and only then stop the
  # sink. Stopping them together lost the spans of a run's last requests.
  sleep 10; kill "${PIDS[1]:-}" 2>/dev/null
  for _ in $(seq 60); do kill -0 "${PIDS[1]:-}" 2>/dev/null || break; sleep 1; done
  kill "${PIDS[@]}" 2>/dev/null; sleep 5; pkill -f "vllm serve"; sleep 10; PIDS=()
  while nvidia-smi --query-compute-apps=pid --format=csv,noheader | grep -q .; do sleep 2; done
}
bench() {   # $1 dir, $2 name, $3 input, $4 seed, $5 rate, $6 max-concurrency (or ""), $7 num-prompts
  local d=$1 name=$2 data=$3 s=$4 rate=$5 c=$6 n=$7
  local mc=(); [ -n "$c" ] && mc=(--max-concurrency "$c")
  curl -s localhost:$PORT/metrics > "$d/$name.metrics_before.txt"
  local t0; t0=$(date +%s.%N)
  # vllm bench serve waits up to 6 h for a response; the longest run of
  # 2026-10-13 took 347 s
  timeout 1800 vllm bench serve --backend vllm --base-url http://127.0.0.1:$PORT --model "$M" \
    --dataset-name custom --dataset-path "$data" --custom-output-len -1 \
    --skip-chat-template --ignore-eos --num-prompts "$n" --request-rate "$rate" --burstiness 1 \
    --seed "$s" "${mc[@]}" --request-id-prefix "$TAG/$(basename "$d")/$name/" \
    --percentile-metrics ttft,tpot,itl,e2el --metric-percentiles 50,90,99 \
    --save-result --save-detailed --result-dir "$d" --result-filename "$name.json" > "$d/$name.log" 2>&1
  local rc=$?
  local t1; t1=$(date +%s.%N)
  curl -s localhost:$PORT/metrics > "$d/$name.metrics_after.txt"
  echo "{\"name\":\"$name\",\"data\":\"$data\",\"seed\":$s,\"rate\":\"$rate\",\"max_concurrency\":\"$c\",\"num_prompts\":$n,\"t_start\":$t0,\"t_end\":$t1,\"rc\":$rc}" >> "$d/index.jsonl"
  return $rc
}
run() {   # bench's arguments: one measured run, logged with its exit code
  local rc=0; bench "$@" || rc=$?
  log "$(basename "$1")/$2 seed=$4 rate=$5 c=$6 n=$7 rc=$rc"
  return $rc
}
