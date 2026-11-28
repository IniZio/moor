FROM node:22@sha256:e13726ca604184d7a91fcacab1fc09eefd642dc28e79ac34e85540dd93b78cfb
ARG HERDR_CORE_VERSION=0.1.16
ARG HERDR_CORE_SHA256=58d6861176dc6fe905b58880d5bff07a9959be613a98e5d66b4583b04b6cfa45
RUN apt-get update && apt-get install -y --no-install-recommends openssh-client curl && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL "https://releases.herdr.dev/core/${HERDR_CORE_VERSION}/herdr-core-linux-x86_64.tar.gz" -o /tmp/hc.tgz \
 && echo "${HERDR_CORE_SHA256}  /tmp/hc.tgz" | sha256sum -c - \
 && tar -xzf /tmp/hc.tgz -C /usr/local/bin herdr-core \
 && ln -s herdr-core /usr/local/bin/hc \
 && rm /tmp/hc.tgz \
 && herdr-core --version
ARG CLAUDE_CODE_VERSION=2.1.326
RUN npm install -g --no-fund --no-audit @anthropic-ai/claude-code@${CLAUDE_CODE_VERSION} && claude --version
COPY bash-wrapper /tmp/bash-wrapper
COPY desk-entry.sh harness publish publish-relay /usr/local/bin/
COPY harness.d/ /usr/local/lib/harness/
COPY instructions.md /usr/local/share/harness/instructions.md
RUN chmod 755 /usr/local/bin/desk-entry.sh /usr/local/bin/harness /usr/local/bin/publish /usr/local/bin/publish-relay /usr/local/lib/harness/* \
 && mv /usr/bin/bash /usr/bin/bash.real && install -m 755 /tmp/bash-wrapper /usr/bin/bash && rm /tmp/bash-wrapper \
 && mkdir -p /workspace /desk /keys /share && chown node /workspace /desk /keys /share
USER node
WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/desk-entry.sh"]
