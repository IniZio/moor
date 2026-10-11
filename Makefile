REPOS ?= $(abspath ..)
OLD_REPO ?= $(REPOS)/moor
OLD_REF ?= b8e8801
OLD_DIR := target/parity-old

DENY_VERSION := 0.20.2
MACHETE_VERSION := 0.9.2
NEXTEST_VERSION := 0.9.148
TAPLO_VERSION := 0.10.0
TYPOS_VERSION := 1.51.1
LLVM_COV_VERSION := 0.9.1

.PHONY: help coverage setup secret audit lint format format-check fmt-check test unit ci build deploy parity-old-bin parity parity-new

help:
	@echo "targets: setup secret audit lint format format-check test ci build deploy parity parity-new"

setup:
	@rustup show active-toolchain >/dev/null
	@cargo deny --version 2>/dev/null | grep -q '$(DENY_VERSION)' || cargo install --locked cargo-deny@$(DENY_VERSION)
	@cargo machete --version 2>/dev/null | grep -q '$(MACHETE_VERSION)' || cargo install --locked cargo-machete@$(MACHETE_VERSION)
	@cargo nextest --version 2>/dev/null | grep -q '$(NEXTEST_VERSION)' || cargo install --locked cargo-nextest@$(NEXTEST_VERSION)
	@taplo --version 2>/dev/null | grep -q '$(TAPLO_VERSION)' || cargo install --locked taplo-cli@$(TAPLO_VERSION)
	@typos --version 2>/dev/null | grep -q '$(TYPOS_VERSION)' || cargo install --locked typos-cli@$(TYPOS_VERSION)
	@cargo llvm-cov --version 2>/dev/null | grep -q '$(LLVM_COV_VERSION)' || cargo install --locked cargo-llvm-cov@$(LLVM_COV_VERSION)
	@echo "setup: toolchain and pinned tools present"

secret:
	@echo "secret: moor holds no repo secrets; nothing to decrypt"

audit:
	cargo deny check
	cargo machete

lint:
	cargo clippy --workspace --all-targets --locked -- -D warnings
	tools/file-cap.sh
	cargo test -p layering --locked
	cargo run -q -p layering --locked
	tools/lint-selftest.sh
	typos
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items --locked

# .editorconfig is honoured through rustfmt.toml and taplo.toml.
format:
	cargo fmt --all
	taplo fmt

format-check:
	cargo fmt --all --check
	taplo fmt --check

fmt-check: format-check

unit:
	cargo nextest run --workspace --lib --locked
	cargo nextest run -p moor --test cli --locked
	cargo test --workspace --doc --locked

test: unit
	$(MAKE) parity

coverage: parity-old-bin
	MOOR_BIN=$(abspath $(OLD_DIR)/moor) MOOR_CLI=old cargo llvm-cov --workspace --locked --ignore-run-fail

ci:
	$(MAKE) audit
	$(MAKE) lint
	$(MAKE) format-check
	$(MAKE) test

build:
	cargo build --workspace --release --locked

deploy:
	@echo "deploy: CLI, nothing to deploy"

parity-old-bin:
	@if [ "$$(cat $(OLD_DIR)/REV 2>/dev/null)" = "$(OLD_REF)" ] && [ -x $(OLD_DIR)/moor ]; then \
		echo "old moor up to date"; \
	else \
		set -e; \
		rm -rf $(OLD_DIR)/src && mkdir -p $(OLD_DIR)/src; \
		git -C $(OLD_REPO) archive $(OLD_REF) | tar -x -C $(OLD_DIR)/src; \
		cargo build --release --locked --manifest-path $(OLD_DIR)/src/Cargo.toml --target-dir $(abspath $(OLD_DIR))/build; \
		cp $(OLD_DIR)/build/release/moor $(OLD_DIR)/moor; \
		printf '%s' '$(OLD_REF)' > $(OLD_DIR)/REV; \
	fi

parity: parity-old-bin
	MOOR_BIN=$(abspath $(OLD_DIR)/moor) MOOR_CLI=old cargo test -p parity --test parity

# Parity suite against the new binary; not part of ci/test (most cases need later slices).
parity-new:
	cargo build -p moor --locked
	MOOR_BIN=$(abspath target/debug/moor) MOOR_CLI=new cargo test -p parity --test parity -- $(PARITY_ARGS)
