#!/bin/bash
# The engine settings of #589, one at a time, at the knee (0.9 x lambda_max of
# the base run), 3 repetitions each. Same workload and client as run_model.sh.
# usage: run_knobs.sh MODEL TAG
set -uo pipefail
M=$1; TAG=$2
cd ~/g; . .venv/bin/activate
export OTEL_EXPORTER_OTLP_TRACES_PROTOCOL=http/protobuf HF_HUB_OFFLINE=1
R=runs/$TAG; PORT=8000; DUR=240
LMAX=$(cat "$R/lambda_max.txt")
RATE=$(python -c "print(round(0.9*$LMAX, 4))"); N=$(python -c "print(min(3000, max(200, round($RATE*$DUR))))")
log() { echo "$(date -u +%FT%TZ) $*" | tee -a "$R/driver.log"; }

BASE=(--dtype bfloat16 --tensor-parallel-size 1 --max-model-len 8192
  --block-size 16 --max-num-batched-tokens 8192 --max-num-seqs 64
  --enable-chunked-prefill --enable-prefix-caching --scheduling-policy fcfs
  --enable-prompt-tokens-details --enable-logging-iteration-details
  --otlp-traces-endpoint http://127.0.0.1:4318/v1/traces --no-async-scheduling)

# name : flags appended to BASE (argparse keeps the last value)
KNOBS=(
  "B512:--max-num-batched-tokens 512"
  "B2048:--max-num-batched-tokens 2048"
  "seqs16:--max-num-seqs 16"
  "seqs128:--max-num-seqs 128"
  "noprefix:--no-enable-prefix-caching"
  "nochunk:--no-enable-chunked-prefill"
  "kv2000:--num-gpu-blocks-override 2000"
)

PIDS=()
stop_server() {
  kill "${PIDS[@]}" 2>/dev/null; sleep 5; pkill -f "vllm serve"; sleep 10; PIDS=()
  while nvidia-smi --query-compute-apps=pid --format=csv,noheader | grep -q .; do sleep 2; done
}
bench() {   # $1 dir, $2 name, $3 seed, $4 rate, $5 num-prompts
  local d=$1 name=$2 s=$3 rate=$4 n=$5
  curl -s localhost:$PORT/metrics > "$d/$name.metrics_before.txt"
  local t0; t0=$(date +%s.%N)
  timeout 1800 vllm bench serve --backend vllm --base-url http://127.0.0.1:$PORT --model "$M" \
    --dataset-name custom --dataset-path "data/$TAG-s$s.jsonl" --custom-output-len -1 \
    --skip-chat-template --ignore-eos --num-prompts "$n" --request-rate "$rate" --burstiness 1 \
    --seed "$s" --request-id-prefix "$TAG/$(basename "$d")/$name/" \
    --percentile-metrics ttft,tpot,itl,e2el --metric-percentiles 50,90,99 \
    --save-result --save-detailed --result-dir "$d" --result-filename "$name.json" > "$d/$name.log" 2>&1
  local rc=$?
  local t1; t1=$(date +%s.%N)
  curl -s localhost:$PORT/metrics > "$d/$name.metrics_after.txt"
  echo "{\"name\":\"$name\",\"seed\":$s,\"rate\":\"$rate\",\"max_concurrency\":\"\",\"num_prompts\":$n,\"t_start\":$t0,\"t_end\":$t1,\"rc\":$rc}" >> "$d/index.jsonl"
  return $rc
}

for k in "${KNOBS[@]}"; do
  name=${k%%:*}; read -r -a extra <<< "${k#*:}"
  d=$R/knob_$name; mkdir -p "$d"
  printf '%s\n' "${BASE[@]}" "${extra[@]}" > "$d/flags.txt"
  python otlp_sink.py 4318 "$d/spans.jsonl" & PIDS+=($!)
  nohup vllm serve "$M" --port $PORT "${BASE[@]}" "${extra[@]}" > "$d/server.log" 2>&1 & PIDS+=($!)
  for _ in $(seq 600); do curl -sf localhost:$PORT/health >/dev/null && break; sleep 2; done
  if ! curl -sf localhost:$PORT/health >/dev/null; then log "knob $name: server did not start"; stop_server; continue; fi
  python metrics.py $PORT "$d/metrics.jsonl" & PIDS+=($!)
  nvidia-smi --query-gpu=timestamp,utilization.gpu,utilization.memory,power.draw,clocks.sm,clocks.mem,temperature.gpu \
    --format=csv,nounits -lms 100 > "$d/gpu.csv" & PIDS+=($!)
  log "knob $name up"
  bench "$d" warmup 1 inf 16 >/dev/null 2>&1 || true
  for rep in 1 2 3; do
    bench "$d" "knee_r$rep" "$rep" "$RATE" "$N"; log "knob $name rep=$rep rate=$RATE n=$N rc=$?"
  done
  stop_server
done
log "KNOBS_DONE $TAG"
