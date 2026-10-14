#!/bin/bash
cd ~/g
bash run_knobs.sh meta-llama/Llama-3.1-8B-Instruct llama31-8b
bash run_knobs.sh Qwen/Qwen3-8B qwen3-8b
echo KNOBS_ALL_DONE
