#!/bin/bash
# Every model, then the manifest. usage: nohup bash run_all.sh > run_all.log 2>&1 &
# Each model has its own servers, so one that fails does not stop the next;
# ALL_DONE only when every one succeeded, else ALL_FAILED and exit 1.
cd ~/g; . .venv/bin/activate; mkdir -p runs
{
  date -u +%FT%TZ; nvidia-smi; nvidia-smi topo -m; nvidia-smi -q | head -80
  python -c 'import vllm, torch; print("vllm", vllm.__version__, "torch", torch.__version__, torch.version.cuda)'
  uname -a; lscpu | head -20; free -g
} > runs/host.txt 2>&1
. ./common.sh
rc=0
for m in "${MODELS[@]}"; do bash run_model.sh "${m%%:*}" "${m#*:}" || rc=1; done
if [ $rc -eq 0 ]; then echo ALL_DONE; else echo ALL_FAILED; fi
exit $rc
