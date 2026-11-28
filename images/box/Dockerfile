FROM docker:29-dind-rootless@sha256:5e9c9ba41ba85de700f2b27e8fca05e85c05c68a5412ac7fd790b07fe039262f
USER root
RUN apk add --no-cache openssh-server openssh-keygen bash git curl jq make coreutils procps findutils grep sed tar xz unzip \
      ca-certificates libstdc++ build-base chromium ffmpeg ttf-dejavu font-noto font-noto-cjk font-liberation \
      postgresql-client \
 && mkdir -p /workspace /cc /keys /opt/mise && chown 1000:1000 /workspace /cc /keys /opt/mise
ARG MISE_VERSION=v2026.11.14
RUN curl -fsSL -o /tmp/m.tgz https://github.com/jdx/mise/releases/download/${MISE_VERSION}/mise-${MISE_VERSION}-linux-x64-musl.tar.gz \
 && tar -xzf /tmp/m.tgz -C /tmp && install -m 755 /tmp/mise/bin/mise /usr/local/bin/mise && rm -rf /tmp/m.tgz /tmp/mise && mise --version
ENV MISE_DATA_DIR=/opt/mise MISE_CONFIG_DIR=/opt/mise/config MISE_STATE_DIR=/opt/mise/state MISE_CACHE_DIR=/home/rootless/.cache/mise \
    MISE_NODE_FLAVOR=musl MISE_NODE_MIRROR_URL=https://unofficial-builds.nodejs.org/download/release/ \
    MISE_PYTHON_PRECOMPILED_OS=unknown-linux-musl MISE_YES=1 MISE_ALL_COMPILE=false MISE_TRUSTED_CONFIG_PATHS=/workspace \
    PATH=/opt/mise/shims:$PATH
USER rootless
RUN mise use -g node@26.12.1 npm:pnpm@10.29.2 uv@0.12.1 python@3.14.2 && mise reshim \
 && node --version && pnpm --version && uv --version && python --version
RUN npm i -g agent-browser@0.45.0 && mise reshim && agent-browser --version
RUN printf '\n[tool_alias]\npnpm = "npm:pnpm"\n\n[settings]\nexec_auto_install = false\nnot_found_auto_install = false\n' >> /opt/mise/config/config.toml && cat /opt/mise/config/config.toml
USER root
RUN docker compose version && docker buildx version && psql --version && ffmpeg -version | head -1 && chromium-browser --version
COPY box-entry.sh /usr/local/bin/box-entry.sh
COPY publish /usr/local/bin/publish
COPY docker-config.json /home/rootless/.docker/config.json
RUN chmod +x /usr/local/bin/box-entry.sh /usr/local/bin/publish && chown -R 1000:1000 /home/rootless/.docker && rm -rf /home/rootless/.cache && mkdir -p /home/rootless/.cache && chown 1000:1000 /home/rootless/.cache
ENV DOCKERD_ROOTLESS_ROOTLESSKIT_MTU=1280
USER rootless
ENTRYPOINT ["/usr/local/bin/box-entry.sh"]
