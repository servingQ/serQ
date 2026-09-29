#!/bin/bash
# serve_pd.sh pull|push NP ND : NP prefillers on GPUs 0.., ND decoders after them, and the proxy on :8000
set -u
MODE=$1; NP=$2; ND=$3
cd ~/pd && . .venv/bin/activate
export PATH=$HOME/.local/bin:$PATH
pkill -f "^vllm serve"; pkill -f "^python toy_proxy_server"; pkill -f "^python disagg_proxy"; sleep 3; pkill -9 -f "EngineCore"; while nvidia-smi --query-compute-apps=pid --format=csv,noheader | grep -q .; do sleep 2; done
export UCX_TLS=cuda_ipc,cuda_copy,tcp,sm VLLM_NIXL_SIDE_CHANNEL_HOST=127.0.0.1 HF_HUB_OFFLINE=1
MODEL=Qwen/Qwen3-8B
if [ "$MODE" = pull ]; then CONN=NixlConnector; else CONN=NixlPushConnector; fi
mkdir -p logs/$MODE
common="--block-size 16 --max-num-batched-tokens 8192 --max-model-len 40960 --enable-prefix-caching --gpu-memory-utilization 0.9 --enable-prompt-tokens-details"
for i in $(seq 0 $((NP-1))); do
  CUDA_VISIBLE_DEVICES=$i VLLM_NIXL_SIDE_CHANNEL_PORT=$((5600+i)) nohup vllm serve $MODEL --port $((8100+i)) --max-num-seqs 16 $common \
    --kv-transfer-config "{\"kv_connector\":\"$CONN\",\"kv_role\":\"kv_producer\",\"engine_id\":\"P$i\",\"kv_load_failure_policy\":\"fail\"}" \
    > logs/$MODE/P$i.log 2>&1 < /dev/null &
done
for j in $(seq 0 $((ND-1))); do
  CUDA_VISIBLE_DEVICES=$((NP+j)) VLLM_NIXL_SIDE_CHANNEL_PORT=$((5700+j)) nohup vllm serve $MODEL --port $((8200+j)) --max-num-seqs 64 $common \
    --kv-transfer-config "{\"kv_connector\":\"$CONN\",\"kv_role\":\"kv_consumer\",\"engine_id\":\"D$j\",\"kv_load_failure_policy\":\"fail\"}" \
    > logs/$MODE/D$j.log 2>&1 < /dev/null &
done
for p in $(seq 8100 $((8100+NP-1))) $(seq 8200 $((8200+ND-1))); do
  for _ in $(seq 1 120); do curl -sf http://127.0.0.1:$p/health >/dev/null && break; sleep 5; done
  curl -sf http://127.0.0.1:$p/health >/dev/null && echo "up :$p" || { echo "DOWN :$p"; tail -5 logs/$MODE/*.log; }
done
if [ "$MODE" = pull ]; then
  PP=$(seq -s' ' 8100 $((8100+NP-1))); DP=$(seq -s' ' 8200 $((8200+ND-1)))
  PH=$(for _ in $(seq 1 $NP); do printf '127.0.0.1 '; done); DH=$(for _ in $(seq 1 $ND); do printf '127.0.0.1 '; done)
  nohup python toy_proxy_server.py --port 8000 --prefiller-hosts $PH --prefiller-ports $PP --decoder-hosts $DH --decoder-ports $DP > logs/$MODE/proxy.log 2>&1 < /dev/null &
else
  nohup python disagg_proxy_pushconnector_demo.py --model $MODEL --prefill localhost:8100 --decode localhost:8200 \
    --prefill-engine-id P0 --prefill-kv-host 127.0.0.1 --prefill-side-channel-port 5600 --prefill-tp-size 1 --prefill-pp-size 1 --port 8000 > logs/$MODE/proxy.log 2>&1 < /dev/null &
fi
sleep 3
echo "proxy: $(curl -s -m 5 http://127.0.0.1:8000/healthcheck || curl -s -m 5 http://127.0.0.1:8000/status | head -c 200)"
echo SERVE_DONE
