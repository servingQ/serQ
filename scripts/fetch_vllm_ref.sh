#!/usr/bin/env bash
# The vLLM source that docs/language.md and tools/ cite by file:line
# (upstream vllm-project/vllm at 0c87a197), checked out into ref/vllm.
# Not a submodule: Cargo would clone it for every git dependency on serQ.
set -euo pipefail
cd "$(dirname "$0")/.."
REV=0c87a197b856a25670cc711456533e5ef9f94d9f
URL=https://github.com/vllm-project/vllm.git

# `--sparse` fetches only the revision and only the files the citations name
# (scripts/check_citations.py --paths, so the two cannot drift): 4 MB and two
# seconds against 228 MB and thirteen for the whole history. CI wants that;
# a person reading upstream wants the full tree.
if [ "${1:-}" = "--sparse" ]; then
  if [ ! -d ref/vllm/.git ]; then
    git init -q ref/vllm
    git -C ref/vllm remote add origin "$URL"
  fi
  git -C ref/vllm fetch -q --depth 1 --filter=blob:none origin "$REV"
  git -C ref/vllm sparse-checkout set --cone $(scripts/check_citations.py --paths)
  git -C ref/vllm -c advice.detachedHead=false checkout -q FETCH_HEAD
  echo "OK: ref/vllm at ${REV:0:8} (sparse)"
  exit 0
fi

if [ ! -d ref/vllm/.git ]; then
  git clone -q --filter=blob:none "$URL" ref/vllm
fi
git -C ref/vllm -c advice.detachedHead=false checkout -q "$REV"
echo "OK: ref/vllm at ${REV:0:8}"
