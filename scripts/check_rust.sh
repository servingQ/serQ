#!/usr/bin/env bash
# The ROUTE interpreter and CLI (crate seq-lang): fmt + clippy + tests (pool
# semantics, the vLLM scheduler scenarios against the upstream oracle in
# tools/oracle), a static check of every program in programs/, and the
# agreement of the three oracles (CPU vLLM = A100 engine; RouteOracle.lean
# generated from the same answers).
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --release --locked
cargo build --release --locked --quiet
n=0
for f in programs/*.route; do
  ./target/release/route check "$f" >/dev/null || { echo "FAIL: $f does not link"; exit 1; }
  n=$((n + 1))
done
python3 scripts/check_oracle_gpu.py
python3 scripts/gen_oracle.py --check
echo "OK: seq-lang, $n programs link, $(ls tools/oracle/*.out.json | wc -l) vLLM oracle scenarios + 1 prefix-cache trace (CPU oracle = A100 engine = ROUTE = Lean)"
