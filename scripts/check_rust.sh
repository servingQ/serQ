#!/usr/bin/env bash
# The serQ interpreter and CLI (crate serq): fmt + clippy + tests (pool
# semantics, the vLLM scheduler scenarios against the upstream oracle in
# tools/oracle), a static check of every program in examples/ and of the
# programs the site's pages show, the agreement of the CPU vLLM oracle with
# the A100 engine, and the language's size against tools/metrics.json.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
# one version, X.Y.Z, for serq and pyserq (a dev version is CI's, not committed)
python3 scripts/version.py check
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy -p pyserq --locked -- -D warnings
cargo test --release --locked
# every test again with debug assertions, which check the pools' memory
# invariant and conservation at every instant (src/engine/interp.rs, #277)
cargo test --locked --quiet
cargo build --release --locked --quiet
./target/release/serq fmt --check examples/*/*.sq docs/tutorial/programs/*.sq docs/tutorial/programs/workloads/*.sq tests/lean-regress/*.sq
# the interpreter's answers the Lean regressions state (lean/Serq/Regress.lean)
python3 scripts/lean_regress.py
# every program with its workload (examples/pairs.json): a program is the
# deployment, the workload a file of its own given when it runs
# (docs/design/paired-workload.md)
python3 scripts/pairs.py --check
n=0
# each under every workload it runs: one deployment under several loads
# meets them only at the server's signature (vllm.sq)
pairs=0
for f in examples/*/*.sq; do
  [ "$(dirname "$f")" = examples/workloads ] && continue
  for w in $(python3 scripts/pairs.py "$f"); do
    ./target/release/serq check "$f" --workload "$w" >/dev/null \
      || { echo "FAIL: $f with $w does not link"; exit 1; }
    for fmt in tikz svg; do
      ./target/release/serq draw "$f" --workload "$w" --format "$fmt" >/dev/null \
        || { echo "FAIL: $f with $w does not draw ($fmt)"; exit 1; }
    done
    pairs=$((pairs + 1))
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
  w=$(python3 scripts/pairs.py "$f" | head -1)
  ./target/release/serq check "$f" --workload "$w" >/dev/null \
    || { echo "FAIL: $f with $w does not link"; exit 1; }
  t=$((t + 1))
done
# The site's own programs, written in the page rather than included: each
# page's "model.sq" and "workload.sq" fences link, and the page's
# `serq … model.sq` commands run on them; any other fence that declares
# `fn main` writes `…` for what it leaves out.
python3 scripts/test_check_docs_examples.py
python3 scripts/check_docs_examples.py
# Experimental runners must explicitly select their execution conditions too.
python3 tools/pd_batching/test_runners.py
# The A100 golden's collector and converter, on made-up input (no GPU).
python3 tools/a100-v031/test_tools.py
python3 scripts/check_oracle_gpu.py
# The docs' `file.py:line` evidence for the vLLM correspondence. Skips loudly
# where ref/vllm is absent (scripts/fetch_vllm_ref.sh --sparse: 4 MB, 2 s).
python3 scripts/check_citations.py
# The language's size (#142): keywords, functions, context variables, IR
# variants and the lines three programs repeat. A change that moves them
# regenerates tools/metrics.json (`make metrics`) and says why in its PR.
python3 scripts/metrics.py --check
# The IR variants no example, tutorial or oracle program uses: a construct
# only tests reach. A change that moves the list regenerates
# tools/unused.json (`make unused`) and says why in its PR.
python3 scripts/unused.py --check
echo "OK: serq, $n programs link and draw under their $pairs workloads, $t tutorial programs link, $(ls tools/oracle/*.out.json | wc -l) vLLM oracle scenarios + 2 prefix-cache traces (CPU oracle = A100 engine = serQ; the shared-prompt trace, CPU oracle = serQ)"
