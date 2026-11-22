# syntax=docker/dockerfile:1.7
# The agent place of a Box: Claude Code, not privileged, owner of the Box's network namespace.
# Build from the repository root: docker build -f images/agent/Dockerfile .
# Every download is pinned by version and sha256, as the controller image's are.

ARG RUST_IMAGE=rust:1.98.1-slim-bookworm@sha256:ff521445a372125ed4f76e1453a1f8098f2d05332d1601d30db1c1f62757e730
ARG NODE_IMAGE=node:22-bookworm-slim@sha256:4b150b48b4116e2a22f2c39cc27025fbf4f153e2078fd3b14d159fdb5c7279da
ARG CLAUDE_CODE_VERSION=2.1.312
ARG HERDR_CORE_VERSION=0.1.16
ARG HERDR_CORE_SHA256_X86_64=58d6861176dc6fe905b58880d5bff07a9959be613a98e5d66b4583b04b6cfa45
ARG HERDR_CORE_SHA256_AARCH64=b6573137d70baff24fb346c957a9879af6bdc4c641188cdf9e47a7b2a677bc92


FROM ${RUST_IMAGE} AS forward
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates musl-tools pkg-config \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add "$(uname -m)-unknown-linux-musl"
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY tools ./tools
ENV CARGO_INCREMENTAL=0
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    target="$(uname -m)-unknown-linux-musl"; \
    export "CC_$(echo "$target" | tr - _)=musl-gcc" CARGO_PROFILE_RELEASE_STRIP=symbols; \
    cargo build --release --locked -p moor-egress-forward --target "$target" \
 && install -D "target/$target/release/moor-egress-forward" /out/moor-egress-forward


FROM ${NODE_IMAGE}
ARG CLAUDE_CODE_VERSION HERDR_CORE_VERSION HERDR_CORE_SHA256_X86_64 HERDR_CORE_SHA256_AARCH64
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git openssh-client socat tinyproxy tini procps util-linux rsync jq nftables \
 && rm -rf /var/lib/apt/lists/* \
 && npm install -g --no-fund --no-audit "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
 && npm cache clean --force \
 && (userdel -r node 2>/dev/null || true) \
 && groupadd --gid 1000 agent \
 && useradd --uid 1000 --gid 1000 --create-home --shell /bin/bash agent \
 && install -d -o agent -g agent /home/agent/moor /run/moor /home/agent/.agents /home/agent/.claude /home/agent/.claude/skills \
      /home/agent/.local /home/agent/.bun \
 && chmod 1777 /tmp

RUN case "$(uname -m)" in \
      x86_64) arch=x86_64; sha=${HERDR_CORE_SHA256_X86_64} ;; \
      aarch64) arch=aarch64; sha=${HERDR_CORE_SHA256_AARCH64} ;; \
      *) echo "herdr-core ${HERDR_CORE_VERSION} has no build for $(uname -m)" >&2; exit 1 ;; \
    esac \
 && curl -fsSL --retry 5 --retry-all-errors --connect-timeout 20 \
      "https://releases.herdr.dev/core/${HERDR_CORE_VERSION}/herdr-core-linux-${arch}.tar.gz" -o /tmp/hc.tar.gz \
 && echo "${sha}  /tmp/hc.tar.gz" | sha256sum -c - \
 && tar -xzf /tmp/hc.tar.gz -C /usr/local/bin herdr-core \
 && ln -s herdr-core /usr/local/bin/hc \
 && rm /tmp/hc.tar.gz \
 && herdr-core --version

COPY --from=forward /out/moor-egress-forward /usr/local/bin/
COPY images/agent/box-shell /usr/local/bin/box-shell
COPY images/agent/moor-agent-init /usr/local/bin/moor-agent-init
COPY images/work/moor-work-init images/work/box-rebuild /usr/local/lib/moor/
COPY images/agent/profile.sh /etc/profile.d/00-moor.sh
COPY images/agent/CLAUDE.md /etc/claude-code/CLAUDE.md

ENV HOME=/home/agent \
    TMPDIR=/run/moor/tmp \
    CLAUDE_CODE_SHELL_PREFIX=/usr/local/bin/box-shell \
    DISABLE_AUTOUPDATER=1
USER agent
WORKDIR /home/agent/moor
ENTRYPOINT ["tini", "--", "/usr/local/bin/moor-agent-init"]
