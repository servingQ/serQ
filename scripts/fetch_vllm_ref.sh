#!/usr/bin/env bash
# The vLLM source that docs/language.md and tools/ cite by file:line
# (upstream vllm-project/vllm at 0c87a197), checked out into ref/vllm.
# Not a submodule: Cargo would clone it for every git dependency on seQ.
set -euo pipefail
cd "$(dirname "$0")/.."
REV=0c87a197b856a25670cc711456533e5ef9f94d9f
if [ ! -d ref/vllm/.git ]; then
  git clone -q --filter=blob:none https://github.com/vllm-project/vllm.git ref/vllm
fi
git -C ref/vllm -c advice.detachedHead=false checkout -q "$REV"
echo "OK: ref/vllm at ${REV:0:8}"
