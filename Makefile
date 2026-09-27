.PHONY: check rust lean oracle
check: rust lean   ## everything CI runs
rust:              ## fmt, clippy, tests, every program links, the oracles agree
	scripts/check_rust.sh
lean:              ## lake build + sorry check + axiom audit
	scripts/check_lean.sh
oracle:            ## regenerate lean/SeQ/RouteOracle.lean from tools/oracle/
	python3 scripts/gen_oracle.py
