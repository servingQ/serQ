#!/bin/bash
# Both models, then the manifest. usage: nohup bash run_all.sh > run_all.log 2>&1 &
cd ~/g; . .venv/bin/activate; mkdir -p runs
{
  date -u +%FT%TZ; nvidia-smi; nvidia-smi topo -m; nvidia-smi -q | head -80
  python -c 'import vllm, torch; print("vllm", vllm.__version__, "torch", torch.__version__, torch.version.cuda)'
  uname -a; lscpu | head -20; free -g
} > runs/host.txt 2>&1
bash run_model.sh meta-llama/Llama-3.1-8B-Instruct llama31-8b
bash run_model.sh Qwen/Qwen3-8B qwen3-8b
echo ALL_DONE
