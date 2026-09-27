SHELL := /bin/bash
export PATH := $(HOME)/.cargo/bin:$(PATH)

.PHONY: check oracle-ir
check:   ## fmt, clippy, tests, every program links, the oracles agree
	scripts/check_rust.sh
oracle-ir:         ## regenerate tools/oracle/*.ir.json (vllm_request.seq per scenario; vllm_replay.seq with cache_trace.csv inlined)
	SEQ_BLESS=1 cargo test --release --test vllm_oracle oracle_ir_files_are_current
	SEQ_BLESS=1 cargo test --release --test vllm_cache cache_ir_file_is_current
