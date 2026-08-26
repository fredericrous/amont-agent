# The CI-parity target is `make check`. Run it before you push.
MAKEFILE_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
INSTALL_BIN_DIR ?= $(HOME)/.local/bin

.PHONY: help check lint test build install uninstall msrv fmt

help:
	@echo "  make check      fmt + clippy + tests — what CI runs"
	@echo "  make lint       fmt --check and clippy -D warnings"
	@echo "  make test       cargo test --locked"
	@echo "  make msrv       build on the floor Cargo.toml claims"
	@echo "  make install    build --release and copy to $(INSTALL_BIN_DIR)"
	@echo "  make uninstall  remove it again"

# EXACTLY the two commands ci.yaml's `rust` job runs, so a contributor can
# reproduce a red job without pushing. If you change a flag here, change it
# there.
lint:
	@cargo fmt --check
	@cargo clippy --all-targets -- -D warnings

test:
	@cargo test --locked

check: lint test
	@echo "  ✓ fmt, clippy and tests — the same three CI runs"

fmt:
	@cargo fmt

# The floor `rust-version` claims, compiled against — the same check ci.yaml
# runs. `--locked` is not optional: a fresh resolve would pick whatever the
# registry offers today and prove nothing about what we ship.
MSRV := 1.85.0
msrv:
	@rustup toolchain list | grep -q '^$(MSRV)' \
	  || rustup toolchain install $(MSRV) --profile minimal --no-self-update
	@cargo +$(MSRV) check --locked --all-targets
	@echo "  ✓ builds on $(MSRV), the floor Cargo.toml claims"

build:
	@cargo build --release --locked

install: build
	@mkdir -p $(INSTALL_BIN_DIR)
	@install -m 0755 $(MAKEFILE_DIR)target/release/amont-agent $(INSTALL_BIN_DIR)/amont-agent
	@echo "installed $(INSTALL_BIN_DIR)/amont-agent"
	@echo "  amont-agent install --write   add the hook to Claude Code settings"
	@echo "  amont-agent doctor            is it installed, runnable, and firing?"
	@echo "  amont-agent backtest          replay your transcripts through the rules"

uninstall:
	@rm -f $(INSTALL_BIN_DIR)/amont-agent
	@echo "removed $(INSTALL_BIN_DIR)/amont-agent"
	@echo "  the settings.json entry is still there: amont-agent uninstall --write"
