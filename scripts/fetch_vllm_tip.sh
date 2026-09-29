#!/usr/bin/env bash
# The files the citations name, at upstream vllm-project/vllm's current
# `main`, into DIR (default tip/vllm) at their repository paths -- what
# `scripts/check_citations.py --tip DIR` reads. Nine raw files over the
# GitHub API, no clone: `gh` must be authenticated (GH_TOKEN in CI).
# `main` is resolved to one commit first, and every file is fetched at that
# commit, so the nine are one snapshot and the line printed names it.
# VLLM_TIP_REF=<sha|branch> fetches another revision (default main): on a
# red day, the suspect commit and its parent, to see which one moved a range.
set -euo pipefail
cd "$(dirname "$0")/.."
DIR="${1:-tip/vllm}"
REF="${VLLM_TIP_REF:-main}"
sha=$(gh api "repos/vllm-project/vllm/commits/$REF" --jq .sha)
for path in $(scripts/check_citations.py --paths); do
  mkdir -p "$DIR/$(dirname "$path")"
  gh api "repos/vllm-project/vllm/contents/$path?ref=$sha" \
    -H "Accept: application/vnd.github.raw+json" > "$DIR/$path"
done
echo "OK: $DIR at vllm-project/vllm@${sha:0:8} ($REF)"
