# syntax=docker/dockerfile:1.7
# The controller image: `moor controller serve` plus every program it runs.
# Every download is pinned by version and sha256 (or by git commit), so a build today and a
# build next month produce the same tools.

ARG RUST_IMAGE=rust:1.98.1-slim-bookworm@sha256:ff521445a372125ed4f76e1453a1f8098f2d05332d1601d30db1c1f62757e730
ARG NODE_IMAGE=node:22-bookworm-slim@sha256:4b150b48b4116e2a22f2c39cc27025fbf4f153e2078fd3b14d159fdb5c7279da
ARG BUN_IMAGE=oven/bun:1.3.14-slim@sha256:d56a2534ffd262e92c12fd3249d3924d296d97086da773f821d7d0477435ea04

ARG CLAUDE_CODE_VERSION=2.1.312

# `build` compiles moor here from source; `prebuilt` copies the context's prebuilt/ (cd.yaml's x86_64 musl build).
ARG BINS=build

# Splits the dependency build from moor's own crates, so the dependencies stay a cached layer.
ARG CARGO_CHEF_VERSION=0.1.78


FROM ${BUN_IMAGE} AS bun


FROM ${NODE_IMAGE} AS web
COPY --from=bun /usr/local/bin/bun /usr/local/bin/bun
WORKDIR /src/crates/moor-gate/web
COPY crates/moor-gate/web/package.json crates/moor-gate/web/bun.lock ./
RUN bun install --frozen-lockfile
COPY ui /src/ui
COPY crates/moor-gate/web ./
RUN bun run build


# moor's own binaries.
FROM ${RUST_IMAGE} AS rust
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl git musl-tools pkg-config \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add "$(uname -m)-unknown-linux-musl"

ARG CARGO_CHEF_VERSION
RUN cargo install --locked "cargo-chef@${CARGO_CHEF_VERSION}"

ENV CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/build/target
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
    pkgs="-p moor -p moor-notify -p moor-machine-runner"; \
    if grep -q '"moor-gate"' recipe.json; then pkgs="$pkgs -p moor-gate"; fi; \
    cargo chef cook --release --locked --recipe-path recipe.json $pkgs

COPY . .
COPY --from=web /src/crates/moor-gate/web/dist crates/moor-gate/web/dist
ENV MOOR_WEB_PREBUILT=1
# The build context carries no .git, so the commit comes in as an argument.
ARG MOOR_COMMIT=unknown
ENV MOOR_COMMIT=${MOOR_COMMIT}
# The release, without its v; empty leaves the build unstamped, so it lets in agents of any release.
ARG MOOR_VERSION=
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git \
    if [ -n "$MOOR_VERSION" ]; then export MOOR_VERSION; else unset MOOR_VERSION; fi; \
    bins="moor moor-notify moor-machine-runner"; \
    if [ -d crates/moor-gate ]; then bins="$bins moor-gate"; fi; \
    cargo build --release --locked $(for b in $bins; do printf -- '-p %s ' "$b"; done) \
 && for b in $bins; do install -D "target/release/$b" "/out/$b"; done
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git \
    target="$(uname -m)-unknown-linux-musl"; \
    export "CC_$(echo "$target" | tr - _)=musl-gcc" CARGO_PROFILE_RELEASE_STRIP=symbols; \
    cargo build --release --locked -p moor-egress-forward --target "$target" \
 && install -D "target/$target/release/moor-egress-forward" /out/moor-egress-forward


FROM scratch AS prebuilt
COPY --chmod=0755 prebuilt/ /out/


FROM ${BINS} AS bins


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

COPY --from=bins /out/ /usr/local/bin/

ENV HOME=/var/lib/moor \
    MOOR_STATE_DIR=/var/lib/moor/state \
    MOOR_CONFIG=/etc/moor/config.yaml \
    DISABLE_AUTOUPDATER=1
USER 10001:10001
WORKDIR /var/lib/moor
ENTRYPOINT ["tini", "--"]
CMD ["moor", "controller", "serve", "--config", "/etc/moor/controller.yaml"]
