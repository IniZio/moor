GOLANGCI_LINT_VERSION := 2.13.2
GOVULNCHECK_VERSION := 1.8.0

# make ci runs test-store when a Docker daemon answers, or when MOOR_TEST_POSTGRES_DSN names a server.
STORE_TESTS := $(if $(or $(MOOR_TEST_POSTGRES_DSN),$(shell docker info >/dev/null 2>&1 && echo docker)),test-store)
# make ci runs test-docker when a Docker daemon answers, unless MOOR_SKIP_DOCKER_TESTS is set.
DOCKER_TESTS := $(if $(MOOR_SKIP_DOCKER_TESTS),,$(shell docker info >/dev/null 2>&1 && echo test-docker))
IMAGES := box desk relay proxy
PORTAL_IMAGE := moor-fresh-portal:dev

.PHONY: setup generate generate-check assets lint lint-selftest format audit test test-store test-docker images ci herdr-golden image

setup:
	@golangci-lint version 2>/dev/null | grep -q 'version $(GOLANGCI_LINT_VERSION) ' || \
		GOTOOLCHAIN=auto go install github.com/golangci/golangci-lint/v2/cmd/golangci-lint@v$(GOLANGCI_LINT_VERSION)
	@govulncheck -version 2>/dev/null | grep -q 'govulncheck@v$(GOVULNCHECK_VERSION)$$' || \
		GOTOOLCHAIN=auto go install golang.org/x/vuln/cmd/govulncheck@v$(GOVULNCHECK_VERSION)
	go mod download
	@echo "setup: golangci-lint $(GOLANGCI_LINT_VERSION), govulncheck $(GOVULNCHECK_VERSION)"

generate:
	go tool templ generate

# generate-check fails when a template and its generated Go file disagree. It leaves the regenerated file in place.
generate-check:
	@sum() { find internal -name '*_templ.go' | sort | xargs -r sha256sum; }; \
	before=$$(sum); go tool templ generate >/dev/null; after=$$(sum); \
	[ "$$before" = "$$after" ] || { echo "generate-check: a template changed its generated file, run make generate and commit it" >&2; exit 1; }

assets:
	cd assets && npm ci && npm run build

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

# Docker tests start real task containers. make test skips them, test-docker runs them and fails on any skip.
test-docker: images
	sh scripts/test-docker.sh

ci: audit generate-check lint lint-selftest test $(STORE_TESTS) $(DOCKER_TESTS)

herdr-golden:
	go test -tags herdrgolden -run TestRecordTranscripts -count=1 ./internal/adapter/herdr/

image:
	docker build -q --network host --provenance=false -f deploy/Dockerfile --build-arg VERSION=$$(git describe --tags --always --dirty) -t $(PORTAL_IMAGE) .
