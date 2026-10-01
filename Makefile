SHELL := /bin/bash
export PATH := $(HOME)/.cargo/bin:$(PATH)

.PHONY: check lean oracle-ir draw-golden citations citation-drift metrics
check:   ## fmt, clippy, tests, every program links, the oracles agree
	scripts/check_rust.sh
lean:              ## the Lean model (lean/, docs/lean.md): oracle theorems current, lake build, no sorry, axiom audit
	PATH=$(HOME)/.elan/bin:$$PATH scripts/check_lean.sh
oracle-ir:         ## regenerate tools/oracle/*.ir.json (vllm_request.sq per scenario; vllm_replay.sq with cache_trace.csv inlined)
	SERQ_BLESS=1 cargo test --release --test vllm_oracle oracle_ir_files_are_current
	SERQ_BLESS=1 cargo test --release --test vllm_cache cache_ir_file_is_current
draw-golden:       ## regenerate tests/golden/*.svg and docs/assets/*.deployment.svg (serq draw)
	SERQ_BLESS=1 cargo test --release --test draw golden_files_are_current
metrics:           ## regenerate tools/metrics.json (the language's size; make check fails when it moves unrecorded)
	scripts/metrics.py --report
citations:         ## re-hash tools/citations.json after re-pointing a citation
	scripts/fetch_vllm_ref.sh --sparse
	scripts/check_citations.py --bless
citation-drift:    ## do the cited vLLM ranges still exist at upstream main? (gh, no clone)
	scripts/fetch_vllm_ref.sh --sparse
	scripts/fetch_vllm_tip.sh tip/vllm
	scripts/check_citations.py --tip tip/vllm
