# syntax=docker/dockerfile:1.7
# The controller image: `moor controller serve` plus every program it runs.
# Every download is pinned by version and sha256 (or by git commit), so a build today and a
# build next month produce the same tools.

ARG RUST_IMAGE=rust:1.98.1-slim-bookworm@sha256:ff521445a372125ed4f76e1453a1f8098f2d05332d1601d30db1c1f62757e730
ARG GO_IMAGE=golang:1.27.3-bookworm@sha256:d9b6a1beb9474adc2a1ce842573639b44b2a0dd49b6669f51d3821baf9ec37c9
ARG NODE_IMAGE=node:22-bookworm-slim@sha256:4b150b48b4116e2a22f2c39cc27025fbf4f153e2078fd3b14d159fdb5c7279da

# The -moor line of crabbox: moor refuses any other build. The tag names the crabbox version
# the commit was released as.
ARG CRABBOX_REPO=https://github.com/IniZio/crabbox.git
ARG CRABBOX_COMMIT=ee5e5205c9d95f54f7420bed587bc88b91efeeb4
ARG CRABBOX_VERSION=0.79.1-moor.17

ARG AGENT37_REPO=https://github.com/IniZio/crabbox-agent37.git
ARG AGENT37_COMMIT=507f3ce84580ee014a24b8f166101cda56f4f376

ARG CLAUDE_CODE_VERSION=2.1.312

# Splits the dependency build from moor's own crates, so the dependencies stay a cached layer.
ARG CARGO_CHEF_VERSION=0.1.78

ARG ZIG_VERSION=0.17.0
ARG ZIG_SHA256_AMD64=1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026
ARG ZIG_SHA256_ARM64=9e8d11661d4ae3bd57702a3832781e23ad151dde5798e16a5ccd503f65234ff8


# The -moor crabbox build, from a pinned commit.
FROM ${GO_IMAGE} AS crabbox
ARG CRABBOX_REPO CRABBOX_COMMIT CRABBOX_VERSION
ENV GOTOOLCHAIN=local CGO_ENABLED=0
RUN git init -q /src \
 && git -C /src fetch -q --depth 1 "$CRABBOX_REPO" "$CRABBOX_COMMIT" \
 && git -C /src checkout -q FETCH_HEAD
WORKDIR /src
RUN --mount=type=cache,target=/go/pkg/mod --mount=type=cache,target=/root/.cache/go-build \
    go build -trimpath \
      -ldflags "-s -w -X github.com/openclaw/crabbox/internal/cli.version=${CRABBOX_VERSION}" \
      -o /out/crabbox ./cmd/crabbox


# moor's own binaries and the agent37 provider.
FROM ${RUST_IMAGE} AS rust
ARG TARGETARCH
ARG ZIG_VERSION ZIG_SHA256_AMD64 ZIG_SHA256_ARM64 AGENT37_REPO AGENT37_COMMIT
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl git musl-tools pkg-config xz-utils \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add "$(uname -m)-unknown-linux-musl"
# Zig builds the terminal emulator that the controller links.
RUN set -eu; \
    case "$TARGETARCH" in \
      amd64) zig_sha=$ZIG_SHA256_AMD64; zig_arch=x86_64 ;; \
      arm64) zig_sha=$ZIG_SHA256_ARM64; zig_arch=aarch64 ;; \
      *) echo "unsupported arch $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/zig.tar.xz "https://ziglang.org/download/${ZIG_VERSION}/zig-${zig_arch}-linux-${ZIG_VERSION}.tar.xz"; \
    echo "$zig_sha  /tmp/zig.tar.xz" | sha256sum -c -; \
    mkdir -p /opt/zig; \
    tar -xJf /tmp/zig.tar.xz -C /opt/zig --strip-components=1; \
    ln -s /opt/zig/zig /usr/local/bin/zig; \
    rm /tmp/zig.tar.xz

RUN git init -q /agent37 \
 && git -C /agent37 fetch -q --depth 1 "$AGENT37_REPO" "$AGENT37_COMMIT" \
 && git -C /agent37 checkout -q FETCH_HEAD

ARG CARGO_CHEF_VERSION
RUN cargo install --locked "cargo-chef@${CARGO_CHEF_VERSION}"

ENV CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/build/target
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/agent37-target \
    cd /agent37 && CARGO_TARGET_DIR=/agent37-target cargo build --release --locked \
 && install -D /agent37-target/release/crabbox-agent37 /out/crabbox-agent37

WORKDIR /build


# The manifests and lockfile, with every source file emptied: the recipe changes only when the
# dependencies do.
FROM rust AS planner
COPY . .
RUN cargo chef prepare --recipe-path /recipe.json


FROM rust AS build
COPY --from=planner /recipe.json recipe.json
# The dependencies, built with the same packages, features and profile as the build below, so
# that build finds them compiled.
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git \
    pkgs="-p moor -p moor-slack -p moor-notify -p moor-machine-runner"; \
    if grep -q '"moor-gate"' recipe.json; then pkgs="$pkgs -p moor-gate"; fi; \
    cargo chef cook --release --locked --recipe-path recipe.json $pkgs

COPY . .
# The build context carries no .git, so the commit comes in as an argument.
ARG MOOR_COMMIT=unknown
ENV MOOR_COMMIT=${MOOR_COMMIT}
# The release, without its v; empty leaves the build unstamped, so it lets in agents of any release.
ARG MOOR_VERSION=
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git \
    if [ -n "$MOOR_VERSION" ]; then export MOOR_VERSION; else unset MOOR_VERSION; fi; \
    bins="moor moor-slack moor-notify moor-machine-runner"; \
    if [ -d crates/moor-gate ]; then bins="$bins moor-gate"; fi; \
    cargo build --release --locked $(for b in $bins; do printf -- '-p %s ' "$b"; done) \
 && for b in $bins; do install -D "target/release/$b" "/out/$b"; done
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git \
    target="$(uname -m)-unknown-linux-musl"; \
    export "CC_$(echo "$target" | tr - _)=musl-gcc" CARGO_PROFILE_RELEASE_STRIP=symbols; \
    cargo build --release --locked -p moor-egress-forward --target "$target" \
 && install -D "target/$target/release/moor-egress-forward" /out/moor-egress-forward


FROM ${NODE_IMAGE} AS runtime
ARG CLAUDE_CODE_VERSION
# tini reaps the process groups the controller starts and passes SIGTERM on for the drain.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates git openssh-client rsync tini \
 && rm -rf /var/lib/apt/lists/* \
 && npm install -g --no-fund --no-audit "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
 && npm cache clean --force \
 && groupadd --gid 10001 moor \
 && useradd --uid 10001 --gid 10001 --home-dir /var/lib/moor --no-create-home --shell /usr/sbin/nologin moor \
 && install -d -o 10001 -g 10001 -m 0700 /var/lib/moor \
 && install -d -m 0755 /etc/moor

COPY --from=crabbox /out/crabbox /usr/local/bin/
COPY --from=rust /out/crabbox-agent37 /usr/local/bin/
COPY --from=build /out/ /usr/local/bin/

ENV HOME=/var/lib/moor \
    MOOR_STATE_DIR=/var/lib/moor/state \
    MOOR_CONFIG=/etc/moor/config.yaml \
    DISABLE_AUTOUPDATER=1
USER 10001:10001
WORKDIR /var/lib/moor
ENTRYPOINT ["tini", "--"]
CMD ["moor", "controller", "serve", "--config", "/etc/moor/controller.yaml"]
