.PHONY: check
check:   ## fmt, clippy, tests, every program links, the oracles agree
	scripts/check_rust.sh
