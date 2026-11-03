DENY_VERSION := 0.20.2
MACHETE_VERSION := 0.9.2
NEXTEST_VERSION := 0.9.148
TAPLO_VERSION := 0.10.0
TYPOS_VERSION := 1.51.1
LLVM_COV_VERSION := 0.9.1
# The pty surface embeds libghostty-vt, whose build runs Zig 0.17 from $ZIG (or PATH)
# and downloads Ghostty's pinned source.
ZIG_VERSION := 0.17.0
ZIG_SHA256 := 1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026
# The same release for the other runners the release build installs it on.
ZIG_SHA256_x86_64-linux := $(ZIG_SHA256)
ZIG_SHA256_aarch64-linux := 9e8d11661d4ae3bd57702a3832781e23ad151dde5798e16a5ccd503f65234ff8
ZIG_SHA256_x86_64-macos := 4f9a1c5269aa17ebda5e6d3c2b89d6cbf36f7d2b22a0306e9ab98f25f95529c6
ZIG_SHA256_aarch64-macos := b607e9b9234790a008116ae5bdb71c6243b84b9fb42a53a9e70fde41c06c536a
ZIG_PLATFORM ?= x86_64-linux
PTY := --features moor/pty-surface

.PHONY: help coverage setup secret audit lint hooks format test unit ci unit-nextest unit-doc build deploy overlap lint-pty unit-pty nested-e2e

help:
	@echo "targets: setup secret audit lint lint-pty format test unit-pty ci build deploy"

setup:
	@rustup show active-toolchain >/dev/null
	@cargo deny --version 2>/dev/null | grep -q '$(DENY_VERSION)' || cargo install --locked cargo-deny@$(DENY_VERSION)
	@cargo machete --version 2>/dev/null | grep -q '$(MACHETE_VERSION)' || cargo install --locked cargo-machete@$(MACHETE_VERSION)
	@cargo nextest --version 2>/dev/null | grep -q '$(NEXTEST_VERSION)' || cargo install --locked cargo-nextest@$(NEXTEST_VERSION)
	@taplo --version 2>/dev/null | grep -q '$(TAPLO_VERSION)' || cargo install --locked taplo-cli@$(TAPLO_VERSION)
	@typos --version 2>/dev/null | grep -q '$(TYPOS_VERSION)' || cargo install --locked typos-cli@$(TYPOS_VERSION)
	@echo "setup: toolchain and pinned tools present"

secret:
	@echo "secret: moor holds no repo secrets; nothing to decrypt"

audit:
	cargo deny check
	cargo machete

lint:
	cargo fmt --all --check
	taplo fmt --check
	cargo clippy --workspace --all-targets --locked -- -D warnings
	tools/file-cap.sh
	tools/removed-names.sh
	tools/box-key.sh
	tools/box-key.sh --selftest
	tools/exec-env.sh
	cargo test -p layering --locked
	cargo run -q -p layering --locked
	tools/lint-selftest.sh
	tools/stack-overlap-selftest.sh
	tools/gitops-bump-selftest.sh
	typos
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items --locked

# Point git at tools/git-hooks, so a push runs the quick lint checks.
hooks:
	git config core.hooksPath tools/git-hooks

# .editorconfig is honoured through rustfmt.toml and taplo.toml.
format:
	cargo fmt --all
	taplo fmt

# One nextest run for every test target of the workspace, so their binaries overlap, and the
# doc tests (which nextest cannot run) alongside it.
unit:
	$(MAKE) --no-print-directory -j2 -O unit-nextest unit-doc

unit-nextest:
	cargo nextest run --workspace --all-targets --locked

unit-doc:
	cargo test --workspace --doc --locked

test: unit

overlap:
	@set -f; tools/stack-overlap.sh --pretty '$(BASE)' $(if $(HEAD),'$(HEAD)') $(if $(FILES),--files $(FILES))

coverage:
	cargo llvm-cov --workspace --locked

ci:
	$(MAKE) audit
	$(MAKE) lint
	$(MAKE) test

build:
	cargo build --workspace --release --locked $(PTY)

lint-pty:
	cargo clippy -p moor --all-targets --locked $(PTY) -- -D warnings

unit-pty:
	cargo test -p moor --lib --locked $(PTY) -- adapters::pty adapters::box_agent::steer wire::

deploy:
	@echo "deploy: CLI, nothing to deploy"

# Needs /dev/kvm and the tools from tools/box-nested-kvm.sh; see docs/development.md.
nested-e2e:
	tools/nested-e2e.sh

# Single source of the pins for CI's install-action step.
.PHONY: print-tool-versions print-zig
print-zig:
	@echo "$(ZIG_VERSION) $(ZIG_SHA256_$(ZIG_PLATFORM))"

print-tool-versions:
	@echo "cargo-deny@$(DENY_VERSION),cargo-machete@$(MACHETE_VERSION),cargo-nextest@$(NEXTEST_VERSION),taplo@$(TAPLO_VERSION),typos@$(TYPOS_VERSION)"
