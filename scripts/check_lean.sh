#!/usr/bin/env bash
# The Lean model of serQ (lean/, docs/lean.md): build it, fail on `sorry`,
# check that the oracle theorems are generated from the current
# tools/oracle/*.ir.json, and audit the axioms. The Lake package is the
# repository root (lakefile.toml), so everything here runs from it.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== one Lake package, at the root =="
# An editor opens a .lean file in the project of the nearest lean-toolchain
# above it and never reads a lakefile's srcDir: a second package file below
# the root splits lean/ and examples/papers/ into two projects, and the
# proofs no longer find `Serq` (#365).
extra=$(git ls-files --cached --others --exclude-standard -- \
  '*lean-toolchain' '*lakefile.toml' '*lakefile.lean' '*lake-manifest.json' | grep / || true)
if [ -n "$extra" ]; then
  echo "FAIL: a Lake package file below the repository root:"; echo "$extra"
  echo "an editor opens a .lean file in the project of the nearest lean-toolchain"
  echo "above it, so the package's files must be the root's alone (#365)"
  exit 1
fi

echo "== oracle theorems current =="
python3 scripts/gen_lean_oracle.py --check
python3 scripts/test_lean_oracle.py
echo "== claims of the paper programs current =="
python3 scripts/gen_lean_claims.py --check

# Mathlib's prebuilt oleans, as CI fetches them before this script
# (.github/workflows/ci.yml, "Mathlib cache"): without them `lake build`
# compiles Mathlib from source, which took a fresh worktree over 18 minutes.
# The stamp holds the toolchain and manifest it was fetched for, so a branch
# that moves Mathlib, and the way back from it, fetch again. Offline it
# succeeds once .lake/packages exists and ~/.cache/mathlib holds the files.
key=$(cat lean-toolchain lake-manifest.json | cksum | cut -d' ' -f1)
stamp=.lake/packages/.serq-cache
if [ "$(cat "$stamp" 2>/dev/null)" != "$key" ]; then
  echo "== Mathlib cache =="
  if lake exe cache get; then
    echo "$key" > "$stamp"
  else
    echo "warning: lake exe cache get failed; lake build may compile Mathlib from source"
  fi
fi
mkdir -p .lake
echo "== lake build =="
lake build 2>&1 | tee .lake/build.log
if grep -q "declaration uses 'sorry'" .lake/build.log; then
  echo "FAIL: a declaration uses sorry"; exit 1
fi
echo "== serq-lean-bench (the executable the bench and DRT run) =="
lake build serq-lean-bench 2>&1 | tail -1
if grep -rn --include='*.lean' -E '\bsorry\b' lean/Serq lean/Serq.lean lean/Bench examples/papers; then
  echo "FAIL: literal 'sorry' found in sources"; exit 1
fi

echo "== axiom audit =="
lake env lean lean/scripts/AxiomAudit.lean 2>&1 | perl -0pe 's/\n[ \t]+/ /g' | tee .lake/axioms.log
BAD=$(sed -n "s/.*depends on axioms: \[\(.*\)\]/\1/p" .lake/axioms.log \
      | tr ',' '\n' | sed 's/^ *//; s/ *$//' \
      | grep -v -E '^(propext|Classical\.choice|Quot\.sound)$' | grep -v '^$' || true)
if [ -n "$BAD" ]; then
  echo "FAIL: non-standard axioms used:"; echo "$BAD"; exit 1
fi
N=$(grep -c "depends on axioms\|does not depend on any axioms" .lake/axioms.log || true)
echo "OK: $N theorems audited; only standard axioms used."
