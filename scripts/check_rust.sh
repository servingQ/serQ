#!/usr/bin/env bash
# The serQ interpreter and CLI (crate serq): fmt + clippy + tests (pool
# semantics, the vLLM scheduler scenarios against the upstream oracle in
# tools/oracle), a static check of every program in examples/, the
# agreement of the CPU vLLM oracle with the A100 engine, and the language's
# size against tools/metrics.json.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
# one version, X.Y.Z, for serq and pyserq (a dev version is CI's, not committed)
python3 scripts/version.py check
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy -p pyserq --locked -- -D warnings
cargo test --release --locked
cargo build --release --locked --quiet
./target/release/serq fmt --check examples/*/*.sq lib/*.sq docs/tutorial/programs/*.sq
n=0
for f in examples/*/*.sq; do
  ./target/release/serq check "$f" >/dev/null || { echo "FAIL: $f does not link"; exit 1; }
  for fmt in tikz svg; do
    ./target/release/serq draw "$f" --format "$fmt" >/dev/null \
      || { echo "FAIL: $f does not draw ($fmt)"; exit 1; }
  done
  n=$((n + 1))
done
for f in tools/oracle/*.ir.json; do
  ./target/release/serq draw "$f" >/dev/null \
    || { echo "FAIL: $f does not draw"; exit 1; }
done
# The tutorial's programs are included into docs/tutorial/*.md with
# pymdownx.snippets, so `mkdocs build --strict` catches a renamed file but not
# one that has stopped linking after a change to the language.
t=0
for f in docs/tutorial/programs/*.sq; do
  ./target/release/serq check "$f" >/dev/null || { echo "FAIL: $f does not link"; exit 1; }
  t=$((t + 1))
done
python3 scripts/check_oracle_gpu.py
# The docs' `file.py:line` evidence for the vLLM correspondence. Skips loudly
# where ref/vllm is absent (scripts/fetch_vllm_ref.sh --sparse: 4 MB, 2 s).
python3 scripts/check_citations.py
# The language's size (#142): keywords, functions, context variables, IR
# variants and the lines three programs repeat. A change that moves them
# regenerates tools/metrics.json (`make metrics`) and says why in its PR.
python3 scripts/metrics.py --check
echo "OK: serq, $n programs link and draw, $t tutorial programs link, $(ls tools/oracle/*.out.json | wc -l) vLLM oracle scenarios + 1 prefix-cache trace (CPU oracle = A100 engine = serQ)"
