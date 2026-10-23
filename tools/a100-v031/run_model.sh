#!/bin/bash
# One model on one GPU: saturation probe, Poisson sweep (3 repetitions), one
# point with async scheduling on. Workload: examples/workloads/single_turn.sq run
# against examples/multi-turn/vllm.sq (examples/single-turn/vllm_single_turn.sq
# at collection time). Every run depends on the ones before it (lambda_max, the
# same server), so the first failure stops the server and exits 1.
# usage: run_model.sh MODEL TAG      (from ~/g, venv active)
set -uo pipefail
M=$1; TAG=$2
cd ~/g; . .venv/bin/activate
R=runs/$TAG; mkdir -p "$R"
. ./common.sh
FRACS="0.2 0.4 0.6 0.8 0.9 1.0 1.2"
CONCS="1 2 4 8 16 32 64 96"

fail() { log "FAILED $TAG"; stop_server; exit 1; }
inputs || { log "FAILED $TAG: inputs"; exit 1; }

# --- sync: probe, then sweep -------------------------------------------------
D=$R/sync; start_server "$D" "${FLAGS[@]}" --no-async-scheduling || exit 1
off=16   # seed 0's first 16 prompts are the warm-up's; the probes take the next 1386
for c in $CONCS; do
  n=$(( c * 6 )); [ $n -lt 30 ] && n=30
  slice "probe_c$c" $off $n || fail; off=$(( off + n ))
  run "$D" "probe_c$c" "data/$TAG-probe_c$c.jsonl" 1 inf "$c" "$n" || fail
done
LMAX=$(python - "$D" <<'EOF'
import json, sys, glob
best = max(json.load(open(f))["request_throughput"] for f in glob.glob(sys.argv[1] + "/probe_c*.json"))
print(f"{best:.4f}")
EOF
) || fail
log "lambda_max=$LMAX"; echo "$LMAX" > "$R/lambda_max.txt"
for rep in 1 2 3; do
  for f in $FRACS; do
    rate=$(python -c "print(round($f*$LMAX, 4))")
    n=$(python -c "print(min(3000, max(200, round($rate*$DUR))))")
    run "$D" "sweep_f${f}_r$rep" "data/$TAG-s$rep.jsonl" "$rep" "$rate" "" "$n" || fail
  done
done
stop_server

# --- async on: one point, for the size of the difference ----------------------
D=$R/async; start_server "$D" "${FLAGS[@]}" --async-scheduling || exit 1
rate=$(python -c "print(round(1.0*$LMAX, 4))"); n=$(python -c "print(min(3000, max(200, round($rate*$DUR))))")
run "$D" "sweep_f1.0_r1" "data/$TAG-s1.jsonl" 1 "$rate" "" "$n" || fail
stop_server
log "DONE $TAG"
