GOLANGCI_LINT_VERSION := 2.13.2
GOVULNCHECK_VERSION := 1.8.0

.PHONY: setup lint lint-selftest format audit test ci herdr-golden

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

ci: audit lint lint-selftest test

herdr-golden:
	go test -tags herdrgolden -run TestRecordTranscripts -count=1 ./internal/adapter/herdr/
