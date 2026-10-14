#!/bin/bash
# Runs on the Lambda instance: vLLM v0.31.0 (release wheel), both models.
# HF_TOKEN must be in the environment (for meta-llama).
set -euo pipefail
cd ~
curl -LsSf https://astral.sh/uv/install.sh | sh >/dev/null 2>&1
export PATH=$HOME/.local/bin:$PATH
mkdir -p ~/g && cd ~/g
uv venv .venv --python 3.12 -q
. .venv/bin/activate
# The PyPI wheel is CUDA 13.0; the 2026-10-13 node had driver 570 (CUDA 12.8),
# on which torch cu130 sees no GPU. The release's CUDA 12.9 wheel runs there.
uv pip install -q --torch-backend=cu129 \
  "https://github.com/vllm-project/vllm/releases/download/v0.31.0/vllm-0.31.0+cu129-cp38-abi3-manylinux_2_28_x86_64.whl" \
  "huggingface_hub[cli]" aiohttp pandas \
  opentelemetry-sdk opentelemetry-api opentelemetry-exporter-otlp opentelemetry-proto
python -c 'import vllm; print("vllm", vllm.__version__)'
python -c 'import torch; assert torch.cuda.is_available(), "no GPU visible"; print("torch", torch.__version__, torch.cuda.get_device_name(0))'
hf download meta-llama/Llama-3.1-8B-Instruct --exclude "original/*" > hf_llama.log 2>&1 && echo llama_ok
hf download Qwen/Qwen3-8B > hf_qwen.log 2>&1 && echo qwen_ok
echo SETUP_DONE
