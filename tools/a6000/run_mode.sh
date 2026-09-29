#!/bin/bash
# run_mode.sh MODE NP ND TAG SPACING : serve, probe lone requests, replay the trace, pack the results.
set -u
MODE=$1; NP=$2; ND=$3; TAG=$4; SP=$5
cd ~/pd && . .venv/bin/activate
pkill -f "^python metrics"; pkill -f "^python pd_replay"
bash serve_pd.sh $MODE $NP $ND > serve_$TAG.log 2>&1
grep -q "DOWN" serve_$TAG.log && { echo "SERVE FAILED"; exit 1; }
mkdir -p runs/$TAG
until curl -s -m 3 http://127.0.0.1:8000/status >/dev/null || curl -s -m 3 http://127.0.0.1:8000/healthcheck >/dev/null; do sleep 2; done
python probe_xfer.py http://127.0.0.1:8000 8200 runs/$TAG/probe.jsonl > runs/$TAG/probe.log 2>&1
echo probe_done
nohup python metrics.py $(seq -s, 8100 $((8100+NP-1))),$(seq -s, 8200 $((8200+ND-1))) runs/$TAG/metrics.jsonl > /dev/null 2>&1 < /dev/null &
MP=$!
python pd_replay.py --csv short_base.csv --base http://127.0.0.1:8000 --model Qwen/Qwen3-8B --spacing $SP --sessions 96 --out runs/$TAG/rounds.jsonl > runs/$TAG/replay.log 2>&1
kill $MP
cp serve_$TAG.log runs/$TAG/; cp logs/$MODE/*.log runs/$TAG/ 2>/dev/null
curl -s http://127.0.0.1:8200/metrics | grep -E "^vllm:nixl_xfer|^vllm:num_preemptions|^vllm:prefix_cache" > runs/$TAG/d_metrics_final.txt
tar czf runs_$TAG.tgz runs/$TAG
echo RUN_DONE
