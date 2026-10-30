# syntax=docker/dockerfile:1.7
# The controller image: `moor controller serve` plus every program it runs.
# Every download is pinned by version and sha256 (or by git commit), so a build today and a
# build next month produce the same tools.

ARG RUST_IMAGE=rust:1.98.1-slim-bookworm@sha256:ff521445a372125ed4f76e1453a1f8098f2d05332d1601d30db1c1f62757e730
ARG GO_IMAGE=golang:1.27.3-bookworm@sha256:d9b6a1beb9474adc2a1ce842573639b44b2a0dd49b6669f51d3821baf9ec37c9
ARG NODE_IMAGE=node:22-bookworm-slim@sha256:4b150b48b4116e2a22f2c39cc27025fbf4f153e2078fd3b14d159fdb5c7279da
ARG FETCH_IMAGE=debian:bookworm-slim@sha256:99c0b25c46d0214d5a2091092e7908907dcaecd6a962cde0ac700acee222b6f3

# The -moor line of crabbox: moor refuses any other build. The tag names the crabbox version
# the commit was released as.
ARG CRABBOX_REPO=https://github.com/IniZio/crabbox.git
ARG CRABBOX_COMMIT=403dadeb5c5bd69ead5e5e10af9c5c6164dc295b
ARG CRABBOX_VERSION=0.79.1-moor.12

ARG AGENT37_REPO=https://github.com/IniZio/crabbox-agent37.git
ARG AGENT37_COMMIT=507f3ce84580ee014a24b8f166101cda56f4f376

ARG CLAUDE_CODE_VERSION=2.1.312

ARG GH_VERSION=2.103.3
ARG GH_SHA256_AMD64=4c806978ffce1cec70853302a56d70ac6da4f19bae7adca4e1a8ab00633dd91d
ARG GH_SHA256_ARM64=ee32df2323c8847f5fac6cb6b4b69ca75828e9329a8f22f677021f7b3306ce63

ARG HERDR_VERSION=0.9.5
ARG HERDR_SHA256_AMD64=14d9e8bc8b4a9e0118d77c32266f76d461f09b40342eb23752960bd89f59929d
ARG HERDR_SHA256_ARM64=17d7536f2da50859523680ef279dffcac8e4a235f4e46fe8da6c2ec44141897c

# Splits the dependency build from moor's own crates, so the dependencies stay a cached layer.
ARG CARGO_CHEF_VERSION=0.1.78

ARG ZIG_VERSION=0.17.0
ARG ZIG_SHA256_AMD64=1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026
ARG ZIG_SHA256_ARM64=9e8d11661d4ae3bd57702a3832781e23ad151dde5798e16a5ccd503f65234ff8


# gh and herdr, checked against their published digests.
FROM ${FETCH_IMAGE} AS fetch
ARG TARGETARCH
ARG GH_VERSION GH_SHA256_AMD64 GH_SHA256_ARM64 HERDR_VERSION HERDR_SHA256_AMD64 HERDR_SHA256_ARM64
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
RUN set -eu; \
    case "$TARGETARCH" in \
      amd64) gh_sha=$GH_SHA256_AMD64; herdr_sha=$HERDR_SHA256_AMD64; herdr_arch=x86_64 ;; \
      arm64) gh_sha=$GH_SHA256_ARM64; herdr_sha=$HERDR_SHA256_ARM64; herdr_arch=aarch64 ;; \
      *) echo "unsupported arch $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    mkdir -p /out; \
    curl -fsSL -o /tmp/gh.tgz "https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_${TARGETARCH}.tar.gz"; \
    echo "$gh_sha  /tmp/gh.tgz" | sha256sum -c -; \
    tar -xzf /tmp/gh.tgz -C /tmp "gh_${GH_VERSION}_linux_${TARGETARCH}/bin/gh"; \
    install -m 0755 "/tmp/gh_${GH_VERSION}_linux_${TARGETARCH}/bin/gh" /out/gh; \
    curl -fsSL -o /tmp/herdr "https://github.com/herdrdev/herdr/releases/download/v${HERDR_VERSION}/herdr-linux-${herdr_arch}"; \
    echo "$herdr_sha  /tmp/herdr" | sha256sum -c -; \
    install -m 0755 /tmp/herdr /out/herdr


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
 && apt-get install -y --no-install-recommends ca-certificates curl git pkg-config xz-utils \
 && rm -rf /var/lib/apt/lists/*
# Zig builds the terminal emulator that the pty surface links.
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
# Comma-separated cargo features of the moor crate. pty-surface lets the controller hold Box
# agent panes itself, with no herdr server.
ARG MOOR_FEATURES=pty-surface
COPY --from=planner /recipe.json recipe.json
# The dependencies, built with the same packages, features and profile as the build below, so
# that build finds them compiled.
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git \
    features=""; \
    if [ -n "$MOOR_FEATURES" ]; then features="--features $(echo "$MOOR_FEATURES" | sed 's#\([^,]\+\)#moor/\1#g')"; fi; \
    pkgs="-p moor -p moor-slack -p moor-machine-runner"; \
    if grep -q '"moor-gate"' recipe.json; then pkgs="$pkgs -p moor-gate"; fi; \
    cargo chef cook --release --locked --recipe-path recipe.json $pkgs $features

COPY . .
# The build context carries no .git, so the commit comes in as an argument.
ARG MOOR_COMMIT=unknown
ENV MOOR_COMMIT=${MOOR_COMMIT}
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git \
    features=""; \
    if [ -n "$MOOR_FEATURES" ]; then features="--features $(echo "$MOOR_FEATURES" | sed 's#\([^,]\+\)#moor/\1#g')"; fi; \
    bins="moor moor-slack moor-machine-runner"; \
    if [ -d crates/moor-gate ]; then bins="$bins moor-gate"; fi; \
    cargo build --release --locked $(for b in $bins; do printf -- '-p %s ' "$b"; done) $features \
 && for b in $bins; do install -D "target/release/$b" "/out/$b"; done


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

COPY --from=fetch /out/gh /out/herdr /usr/local/bin/
COPY --from=crabbox /out/crabbox /usr/local/bin/
COPY --from=rust /out/crabbox-agent37 /usr/local/bin/
COPY --from=build /out/ /usr/local/bin/

ENV HOME=/var/lib/moor \
    MOOR_STATE_DIR=/var/lib/moor/state \
    MOOR_CONFIG=/etc/moor/config.yaml \
    MOOR_SURFACE=pty \
    DISABLE_AUTOUPDATER=1
USER 10001:10001
WORKDIR /var/lib/moor
ENTRYPOINT ["tini", "--"]
CMD ["moor", "controller", "serve", "--config", "/etc/moor/controller.yaml"]
