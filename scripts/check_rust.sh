#!/usr/bin/env bash
# The seQ interpreter and CLI (crate seq-lang): fmt + clippy + tests (pool
# semantics, the vLLM scheduler scenarios against the upstream oracle in
# tools/oracle), a static check of every program in programs/, and the
# agreement of the CPU vLLM oracle with the A100 engine.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --release --locked
cargo build --release --locked --quiet
n=0
for f in programs/*.seq; do
  ./target/release/seq-lang check "$f" >/dev/null || { echo "FAIL: $f does not link"; exit 1; }
  for v in deployment route; do
    for fmt in tikz svg; do
      ./target/release/seq-lang draw "$f" --view "$v" --format "$fmt" >/dev/null \
        || { echo "FAIL: $f does not draw ($v/$fmt)"; exit 1; }
    done
  done
  n=$((n + 1))
done
for f in tools/oracle/*.ir.json; do
  ./target/release/seq-lang draw "$f" --view deployment >/dev/null \
    || { echo "FAIL: $f does not draw"; exit 1; }
done
python3 scripts/check_oracle_gpu.py
echo "OK: seq-lang, $n programs link and draw, $(ls tools/oracle/*.out.json | wc -l) vLLM oracle scenarios + 1 prefix-cache trace (CPU oracle = A100 engine = seQ)"
