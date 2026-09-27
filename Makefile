.PHONY: check oracle-ir
check:   ## fmt, clippy, tests, every program links, the oracles agree
	scripts/check_rust.sh
oracle-ir:         ## regenerate tools/oracle/*.ir.json from programs/vllm_request.seq and the scenarios
	SEQ_BLESS=1 cargo test --release --test vllm_oracle oracle_ir_files_are_current
