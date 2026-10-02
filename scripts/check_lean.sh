#!/usr/bin/env bash
# The Lean model of serQ (lean/, docs/lean.md): build it, fail on `sorry`,
# check that the oracle theorems are generated from the current
# tools/oracle/*.ir.json, and audit the axioms.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== oracle theorems current =="
python3 scripts/gen_lean_oracle.py --check
python3 scripts/test_lean_oracle.py

cd lean
echo "== lake build =="
lake build 2>&1 | tee build.log
if grep -q "declaration uses 'sorry'" build.log; then
  echo "FAIL: a declaration uses sorry"; exit 1
fi
echo "== serq-lean-bench (the executable the bench and DRT run) =="
lake build serq-lean-bench 2>&1 | tail -1
if grep -rn --include='*.lean' -E '\bsorry\b' Serq Serq.lean Bench; then
  echo "FAIL: literal 'sorry' found in sources"; exit 1
fi

echo "== axiom audit =="
lake env lean scripts/AxiomAudit.lean 2>&1 | perl -0pe 's/\n[ \t]+/ /g' | tee axioms.log
BAD=$(sed -n "s/.*depends on axioms: \[\(.*\)\]/\1/p" axioms.log \
      | tr ',' '\n' | sed 's/^ *//; s/ *$//' \
      | grep -v -E '^(propext|Classical\.choice|Quot\.sound)$' | grep -v '^$' || true)
if [ -n "$BAD" ]; then
  echo "FAIL: non-standard axioms used:"; echo "$BAD"; exit 1
fi
N=$(grep -c "depends on axioms\|does not depend on any axioms" axioms.log || true)
echo "OK: $N theorems audited; only standard axioms used."
