SHELL := /bin/bash
export PATH := $(HOME)/.cargo/bin:$(PATH)

.PHONY: check oracle-ir draw-golden citations citation-drift
check:   ## fmt, clippy, tests, every program links, the oracles agree
	scripts/check_rust.sh
oracle-ir:         ## regenerate tools/oracle/*.ir.json (vllm_request.seq per scenario; vllm_replay.seq with cache_trace.csv inlined)
	SEQ_BLESS=1 cargo test --release --test vllm_oracle oracle_ir_files_are_current
	SEQ_BLESS=1 cargo test --release --test vllm_cache cache_ir_file_is_current
draw-golden:       ## regenerate tests/golden/*.svg (seq-lang draw)
	SEQ_BLESS=1 cargo test --release --test draw golden_files_are_current
citations:         ## re-hash tools/citations.json after re-pointing a citation
	scripts/fetch_vllm_ref.sh --sparse
	scripts/check_citations.py --bless
citation-drift:    ## do the cited vLLM ranges still exist at upstream main? (gh, no clone)
	scripts/fetch_vllm_ref.sh --sparse
	scripts/fetch_vllm_tip.sh tip/vllm
	scripts/check_citations.py --tip tip/vllm
