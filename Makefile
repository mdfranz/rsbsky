BIN := rsbsky
# Go baseline binary for capture / diff / roundtrip (build it with `make -C ../bskyban`)
GO_BIN ?= ../bskyban/bskyban

.PHONY: all build release run test check fmt clippy compat compat-diff compat-capture tui-check telemetry-check roundtrip install clean help

all: fmt clippy build ## format, lint, build

build: ## debug build
	cargo build

release: ## optimised build: target/release/rsbsky
	cargo build --release

run: ## launch the feed TUI (ARGS="--refresh 0 --fancy")
	cargo run -- $(ARGS)

test: ## unit, wiremock, compat-matrix and terminal-bytes tests
	cargo test

check: ## what CI runs: fmt --check, clippy -D warnings, tests
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	cargo test

fmt: ## rustfmt all files
	cargo fmt

clippy: ## lint, warnings are errors
	cargo clippy --all-targets -- -D warnings

compat: build ## compare against the Go golden outputs (needs python3, not Go)
	python3 scripts/compat.py check --bin target/debug/$(BIN)

compat-diff: build ## run Go and Rust live and compare (needs GO_BIN)
	python3 scripts/compat.py diff --go-bin $(GO_BIN) --bin target/debug/$(BIN)

compat-capture: ## refresh tests/compat/golden/ from the Go baseline (needs GO_BIN)
	python3 scripts/compat.py capture --go-bin $(GO_BIN)

tui-check: ## slow real-terminal suite: tmux + pty (add GO_BIN to also check Go reads our audit log)
	cargo build --examples
	python3 scripts/tui_check.py --bin target/debug/$(BIN) --panic-bin target/debug/examples/panic_restore $(if $(wildcard $(GO_BIN)),--go-bin $(GO_BIN),)

telemetry-check: ## Logfire export vs a fake collector, with and without the `logfire` feature
	cargo build
	python3 scripts/telemetry_check.py --bin target/debug/rsbsky
	CARGO_TARGET_DIR=target/no-logfire cargo build --no-default-features
	python3 scripts/telemetry_check.py --bin target/no-logfire/debug/rsbsky --disabled

roundtrip: build ## Go <-> Rust round trips over shared state/audit files (needs GO_BIN)
	python3 scripts/roundtrip.py --go-bin $(GO_BIN) --bin target/debug/$(BIN)

install: ## install to ~/.cargo/bin
	cargo install --path .

clean: ## remove build output
	cargo clean
	find scripts -name __pycache__ -prune -exec rm -rf {} +

help: ## list targets
	@grep -E '^[a-z-]+:.*##' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-15s %s\n", $$1, $$2}'
