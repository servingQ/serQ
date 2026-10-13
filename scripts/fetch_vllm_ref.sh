#!/usr/bin/env bash
# The vLLM source that docs/language.md and tools/ cite by file:line
# (upstream vllm-project/vllm at 0c87a197), checked out into ref/vllm.
# Not a submodule: Cargo would clone it for every git dependency on serQ.
set -euo pipefail
cd "$(dirname "$0")/.."
REV=0c87a197b856a25670cc711456533e5ef9f94d9f
URL=https://github.com/vllm-project/vllm.git

# The release tags some files are cited at instead of REV (the LMCache
# connector at v0.31.0: scripts/check_citations.py, RELEASED), fetched as
# tags so that `git show TAG:path` reads them.
fetch_releases() {
  for tag in $(scripts/check_citations.py --releases); do
    git -C ref/vllm fetch -q --depth 1 "$@" origin "refs/tags/$tag:refs/tags/$tag"
  done
}

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
  fetch_releases --filter=blob:none
  echo "OK: ref/vllm at ${REV:0:8} (sparse)"
  exit 0
fi

if [ ! -d ref/vllm/.git ]; then
  git clone -q --filter=blob:none "$URL" ref/vllm
fi
git -C ref/vllm -c advice.detachedHead=false checkout -q "$REV"
fetch_releases
echo "OK: ref/vllm at ${REV:0:8}"
