#!/bin/bash
# The engine settings of #589, one at a time, at the knee (0.9 x lambda_max of
# the base run), 3 repetitions each. Same workload and client as run_model.sh.
# Each setting has its own server, so a failure ends that setting only; the
# script goes on with the next and exits 1 at the end.
# usage: run_knobs.sh MODEL TAG
set -uo pipefail
M=$1; TAG=$2
cd ~/g; . .venv/bin/activate
R=runs/$TAG
. ./common.sh
LMAX=$(cat "$R/lambda_max.txt") || exit 1
RATE=$(python -c "print(round(0.9*$LMAX, 4))"); N=$(python -c "print(min(3000, max(200, round($RATE*$DUR))))")

# name : flags appended to FLAGS (argparse keeps the last value)
KNOBS=(
  "B512:--max-num-batched-tokens 512"
  "B2048:--max-num-batched-tokens 2048"
  "seqs16:--max-num-seqs 16"
  "seqs128:--max-num-seqs 128"
  "noprefix:--no-enable-prefix-caching"
  "nochunk:--no-enable-chunked-prefill"
  "kv2000:--num-gpu-blocks-override 2000"
)

inputs || { log "KNOBS_FAILED $TAG: inputs"; exit 1; }
failed=0
for k in "${KNOBS[@]}"; do
  name=${k%%:*}; read -r -a extra <<< "${k#*:}"
  d=$R/knob_$name
  start_server "$d" "${FLAGS[@]}" --no-async-scheduling "${extra[@]}" || { failed=1; continue; }
  for rep in 1 2 3; do
    run "$d" "knee_r$rep" "data/$TAG-s$rep.jsonl" "$rep" "$RATE" "" "$N" || { failed=1; break; }
  done
  stop_server
done
if [ $failed -eq 0 ]; then log "KNOBS_DONE $TAG"; else log "KNOBS_FAILED $TAG"; fi
exit $failed
