#!/bin/bash
# One model on one GPU: saturation probe, Poisson sweep (3 repetitions), one
# point with async scheduling on. Workload: examples/single-turn/vllm_single_turn.sq.
# usage: run_model.sh MODEL TAG      (from ~/g, venv active)
set -uo pipefail
M=$1; TAG=$2
cd ~/g; . .venv/bin/activate
R=runs/$TAG; mkdir -p "$R" data
export OTEL_EXPORTER_OTLP_TRACES_PROTOCOL=http/protobuf HF_HUB_OFFLINE=1
PORT=8000
DUR=240                       # seconds of arrivals per sweep point
FRACS="0.2 0.4 0.6 0.8 0.9 1.0 1.2"
CONCS="1 2 4 8 16 32 64 96"

log() { echo "$(date -u +%FT%TZ) $*" | tee -a "$R/driver.log"; }

for s in 1 2 3; do
  [ -f "data/$TAG-s$s.jsonl" ] || python gen_dataset.py "$M" 3000 "$s" "data/$TAG-s$s.jsonl" >> "$R/driver.log" 2>&1
done

FLAGS=(--dtype bfloat16 --tensor-parallel-size 1 --max-model-len 8192
  --block-size 16 --max-num-batched-tokens 8192 --max-num-seqs 64
  --enable-chunked-prefill --enable-prefix-caching --scheduling-policy fcfs
  --enable-prompt-tokens-details --enable-logging-iteration-details
  --otlp-traces-endpoint http://127.0.0.1:4318/v1/traces)

PIDS=()
start_server() {   # $1 = sync|async, $2 = server dir
  local mode=$1 d=$2; mkdir -p "$d"
  local a=--no-async-scheduling; [ "$mode" = async ] && a=--async-scheduling
  printf '%s\n' "${FLAGS[@]}" "$a" > "$d/flags.txt"
  python otlp_sink.py 4318 "$d/spans.jsonl" & PIDS+=($!)
  nohup vllm serve "$M" --port $PORT "${FLAGS[@]}" "$a" > "$d/server.log" 2>&1 & PIDS+=($!)
  for _ in $(seq 600); do curl -sf localhost:$PORT/health >/dev/null && break; sleep 2; done
  curl -sf localhost:$PORT/health >/dev/null || { log "server did not start"; return 1; }
  python metrics.py $PORT "$d/metrics.jsonl" & PIDS+=($!)
  nvidia-smi --query-gpu=timestamp,utilization.gpu,utilization.memory,power.draw,clocks.sm,clocks.mem,temperature.gpu \
    --format=csv,nounits -lms 100 > "$d/gpu.csv" & PIDS+=($!)
  log "server $mode up"
  # warm-up, discarded
  bench "$d" warmup 1 inf 4 16 >/dev/null 2>&1 || true
}
stop_server() {
  kill "${PIDS[@]}" 2>/dev/null; sleep 5; pkill -f "vllm serve" ; sleep 10; PIDS=()
  while nvidia-smi --query-compute-apps=pid --format=csv,noheader | grep -q .; do sleep 2; done
}
bench() {   # $1 dir, $2 name, $3 seed, $4 rate, $5 max-concurrency (or ""), $6 num-prompts
  local d=$1 name=$2 s=$3 rate=$4 c=$5 n=$6
  local mc=(); [ -n "$c" ] && mc=(--max-concurrency "$c")
  curl -s localhost:$PORT/metrics > "$d/$name.metrics_before.txt"
  local t0; t0=$(date +%s.%N)
  vllm bench serve --backend vllm --base-url http://127.0.0.1:$PORT --model "$M" \
    --dataset-name custom --dataset-path "data/$TAG-s$s.jsonl" --custom-output-len -1 \
    --skip-chat-template --ignore-eos --num-prompts "$n" --request-rate "$rate" --burstiness 1 \
    --seed "$s" "${mc[@]}" --request-id-prefix "$TAG/$name/" \
    --percentile-metrics ttft,tpot,itl,e2el --metric-percentiles 50,90,99 \
    --save-result --save-detailed --result-dir "$d" --result-filename "$name.json" > "$d/$name.log" 2>&1
  local rc=$?
  local t1; t1=$(date +%s.%N)
  curl -s localhost:$PORT/metrics > "$d/$name.metrics_after.txt"
  echo "{\"name\":\"$name\",\"seed\":$s,\"rate\":\"$rate\",\"max_concurrency\":\"$c\",\"num_prompts\":$n,\"t_start\":$t0,\"t_end\":$t1,\"rc\":$rc}" >> "$d/index.jsonl"
  return $rc
}

# --- sync: probe, then sweep -------------------------------------------------
D=$R/sync; start_server sync "$D" || exit 1
for c in $CONCS; do
  n=$(( c * 6 )); [ $n -lt 30 ] && n=30
  bench "$D" "probe_c$c" 1 inf "$c" "$n"; log "probe c=$c rc=$?"
done
LMAX=$(python - "$D" <<'EOF'
import json, sys, glob
best = max(json.load(open(f))["request_throughput"] for f in glob.glob(sys.argv[1] + "/probe_c*.json"))
print(f"{best:.4f}")
EOF
)
log "lambda_max=$LMAX"; echo "$LMAX" > "$R/lambda_max.txt"
for rep in 1 2 3; do
  for f in $FRACS; do
    rate=$(python -c "print(round($f*$LMAX, 4))")
    n=$(python -c "print(min(3000, max(200, round($rate*$DUR))))")
    bench "$D" "sweep_f${f}_r$rep" "$rep" "$rate" "" "$n"; log "sweep f=$f rep=$rep rate=$rate n=$n rc=$?"
  done
done
stop_server

# --- async on: one point, for the size of the difference ----------------------
D=$R/async; start_server async "$D" || exit 1
rate=$(python -c "print(round(1.0*$LMAX, 4))"); n=$(python -c "print(min(3000, max(200, round($rate*$DUR))))")
bench "$D" "sweep_f1.0_r1" 1 "$rate" "" "$n"; log "async f=1.0 rc=$?"
stop_server
log "DONE $TAG"
