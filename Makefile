DENY_VERSION := 0.20.2
MACHETE_VERSION := 0.9.2
NEXTEST_VERSION := 0.9.148
TAPLO_VERSION := 0.10.0
TYPOS_VERSION := 1.51.1
BINSTALL_VERSION := 1.26.2
LLVM_COV_VERSION := 0.9.1
ZIG_VERSION := 0.17.0
HELM_VERSION := 3.21.3
ZIG_SHA256 := 1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026
# The same release for the other runners the release build installs it on.
ZIG_SHA256_x86_64-linux := $(ZIG_SHA256)
ZIG_SHA256_aarch64-linux := 9e8d11661d4ae3bd57702a3832781e23ad151dde5798e16a5ccd503f65234ff8
ZIG_SHA256_x86_64-macos := 4f9a1c5269aa17ebda5e6d3c2b89d6cbf36f7d2b22a0306e9ab98f25f95529c6
ZIG_SHA256_aarch64-macos := b607e9b9234790a008116ae5bdb71c6243b84b9fb42a53a9e70fde41c06c536a
ZIG_PLATFORM ?= x86_64-linux
PTY := --features moor/pty-surface

PG_URL ?= postgres://moor:moor@127.0.0.1:5432/moor

.PHONY: help coverage setup secret audit lint prepush hooks format test unit ci unit-nextest unit-doc build deploy overlap lint-pty unit-pty nested-e2e chart dev-db unit-pg

help:
	@echo "targets: setup secret audit lint prepush hooks chart lint-pty format test unit-pty unit-pg dev-db ci build deploy"

setup:
	@rustup show active-toolchain >/dev/null
	@cargo binstall -V 2>/dev/null | grep -qx '$(BINSTALL_VERSION)' || curl --retry 5 -fsSL --proto '=https' --tlsv1.2 https://raw.githubusercontent.com/cargo-bins/cargo-binstall/v$(BINSTALL_VERSION)/install-from-binstall-release.sh | BINSTALL_VERSION=$(BINSTALL_VERSION) sh
	@cargo deny --version 2>/dev/null | grep -q '$(DENY_VERSION)' || cargo binstall --no-confirm --locked --disable-strategies quick-install cargo-deny@=$(DENY_VERSION)
	@cargo machete --version 2>/dev/null | grep -q '$(MACHETE_VERSION)' || cargo binstall --no-confirm --locked --disable-strategies quick-install cargo-machete@=$(MACHETE_VERSION)
	@cargo nextest --version 2>/dev/null | grep -q '$(NEXTEST_VERSION)' || cargo binstall --no-confirm --locked --disable-strategies quick-install cargo-nextest@=$(NEXTEST_VERSION)
	@taplo --version 2>/dev/null | grep -q '$(TAPLO_VERSION)' || cargo binstall --no-confirm --locked --disable-strategies quick-install taplo-cli@=$(TAPLO_VERSION)
	@typos --version 2>/dev/null | grep -q '$(TYPOS_VERSION)' || cargo binstall --no-confirm --locked --disable-strategies quick-install typos-cli@=$(TYPOS_VERSION)
	@helm version --short 2>/dev/null | grep -q '^v$(HELM_VERSION)+' || echo "setup: make chart needs helm $(HELM_VERSION)"
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
	tools/release-features.sh
	tools/box-key.sh
	tools/box-key.sh --selftest
	tools/exec-env.sh
	cargo test -p layering --locked
	cargo run -q -p layering --locked
	cargo test -p test-children --locked
	cargo run -q -p test-children --locked
	cargo test -p migration-numbers --locked
	cargo run -q -p migration-numbers --locked -- $(or $(BASE),origin/develop)
	cargo test -p store-sql --locked
	cargo run -q -p store-sql --locked
	tools/lint-selftest.sh
	tools/stack-overlap-selftest.sh
	tools/gitops-bump-selftest.sh
	typos
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items --locked

# The chart with only its required values, then with every optional block on.
CHART := deploy/helm/moor
CHART_VALUES := -f deploy/helm/required-values.yaml
chart:
	@command -v helm >/dev/null || { echo "chart: helm $(HELM_VERSION) is not installed"; exit 1; }
	helm lint --strict $(CHART) $(CHART_VALUES)
	helm lint --strict $(CHART) $(CHART_VALUES) -f deploy/helm/full-values.yaml
	helm template moor $(CHART) $(CHART_VALUES) >/dev/null
	helm template moor $(CHART) $(CHART_VALUES) -f deploy/helm/full-values.yaml >/dev/null
	helm template moor $(CHART) $(CHART_VALUES) -f deploy/helm/full-values.yaml \
	  --set gate.ingress.enabled=false --set egress.url= 2>&1 | grep -q 'egress.enabled needs gate.ingress.enabled'
	helm template moor $(CHART) $(CHART_VALUES) -f deploy/helm/full-values.yaml \
	  --set egress.url=ws://egress.test/egress 2>&1 | grep -q 'egress.url .* must be a wss://'
	helm template moor $(CHART) $(CHART_VALUES) -f deploy/helm/full-values.yaml \
	  --set egress.ledgerTls.ca= 2>&1 | grep -q 'egress.ledgerTls needs cert, key and ca'
	helm template moor $(CHART) $(CHART_VALUES) -f deploy/helm/full-values.yaml \
	  --set egress.ledgerTls.runnerClientKey= 2>&1 | grep -q 'egress.ledgerTls with your own ca needs caKey'
	! helm template moor $(CHART) $(CHART_VALUES) -f deploy/helm/full-values.yaml \
	  | grep -E '^(apiVersion: policy/|kind: (NetworkPolicy|PodDisruptionBudget))'
	! helm template moor $(CHART) $(CHART_VALUES) -f deploy/helm/full-values.yaml \
	  --set gate.ingress.enabled=false | grep -E '^apiVersion: (networking\.k8s\.io|policy)/'

# The two gates that CI runs before any test, so a push that fails one never reaches the tests.
prepush:
	$(MAKE) --no-print-directory audit
	$(MAKE) --no-print-directory lint

# Opt in to running `make prepush` before every `git push`.
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

dev-db:
	docker compose up -d --wait postgres

unit-pg:
	MOOR_TEST_POSTGRES_URL='$(PG_URL)' cargo nextest run --workspace --all-targets --locked --run-ignored only -E 'package(moor-schema)'
	MOOR_TEST_POSTGRES_URL='$(PG_URL)' cargo nextest run --locked -p moor --lib -E 'test(/^store::/) - test(/^store::db::sqlite::/)'

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

nested-e2e:
	tools/nested-e2e.sh

# Single source of the pins for CI's install-action step.
.PHONY: print-tool-versions print-zig print-helm
print-helm:
	@echo "$(HELM_VERSION)"

print-zig:
	@echo "$(ZIG_VERSION) $(ZIG_SHA256_$(ZIG_PLATFORM))"

print-tool-versions:
	@echo "cargo-deny@$(DENY_VERSION),cargo-machete@$(MACHETE_VERSION),cargo-nextest@$(NEXTEST_VERSION),taplo@$(TAPLO_VERSION),typos@$(TYPOS_VERSION)"
