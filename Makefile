GOLANGCI_LINT_VERSION := 2.13.2
GOVULNCHECK_VERSION := 1.8.0

# make ci runs test-store when a Docker daemon answers, or when MOOR_TEST_POSTGRES_DSN names a server.
STORE_TESTS := $(if $(or $(MOOR_TEST_POSTGRES_DSN),$(shell docker info >/dev/null 2>&1 && echo docker)),test-store)
IMAGES := box desk relay proxy

.PHONY: setup lint lint-selftest format audit test test-store images ci herdr-golden

setup:
	@golangci-lint version 2>/dev/null | grep -q 'version $(GOLANGCI_LINT_VERSION) ' || \
		GOTOOLCHAIN=auto go install github.com/golangci/golangci-lint/v2/cmd/golangci-lint@v$(GOLANGCI_LINT_VERSION)
	@govulncheck -version 2>/dev/null | grep -q 'govulncheck@v$(GOVULNCHECK_VERSION)$$' || \
		GOTOOLCHAIN=auto go install golang.org/x/vuln/cmd/govulncheck@v$(GOVULNCHECK_VERSION)
	go mod download
	@echo "setup: golangci-lint $(GOLANGCI_LINT_VERSION), govulncheck $(GOVULNCHECK_VERSION)"

lint:
	golangci-lint fmt --diff ./...
	go vet ./...
	golangci-lint run ./...

lint-selftest:
	sh lint/selftest.sh

format:
	golangci-lint fmt ./...

audit:
	govulncheck ./...
	go mod verify
	go mod tidy -diff

test:
	go test -race -count=1 ./...

# Store tests need a Postgres. make test skips them, test-store runs them and fails on any skip.
test-store:
	sh scripts/test-store.sh

images:
	@for i in $(IMAGES); do docker build -q --network host -t moor-fresh-$$i:dev images/$$i || exit 1; done
	@echo "images: built $(IMAGES:%=moor-fresh-%:dev)"

ci: audit lint lint-selftest test $(STORE_TESTS)

herdr-golden:
	go test -tags herdrgolden -run TestRecordTranscripts -count=1 ./internal/adapter/herdr/
