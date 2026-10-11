REPOS ?= $(abspath ..)
OLD_REPO ?= $(REPOS)/moor
OLD_REF ?= b8e8801
OLD_DIR := target/parity-old

.PHONY: parity-old-bin parity fmt-check lint ci

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

fmt-check:
	cargo fmt --all --check

lint:
	cargo clippy --all-targets -- -D warnings

ci: fmt-check lint parity
