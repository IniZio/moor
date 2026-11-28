FROM alpine:3.22@sha256:49340b346798875b5592e5d420241f2f9cfba5f47e62ebb1446a1b34734e898f
ARG NONO_VERSION=0.82.0
ARG NONO_SHA256=2d81384d9025c7ff9c9e29b8f5e83d39cc6739ba37ba8a0c50fe50ceea4653de
RUN wget -q -O /tmp/nono.tgz "https://github.com/nolabs-ai/nono/releases/download/v${NONO_VERSION}/nono-v${NONO_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
 && echo "${NONO_SHA256}  /tmp/nono.tgz" | sha256sum -c - \
 && tar -xzf /tmp/nono.tgz -C /usr/local/bin nono \
 && rm /tmp/nono.tgz \
 && nono --version
ENV HOME=/tmp
USER 65534
ENTRYPOINT ["nono","proxy","--listen","0.0.0.0","--port","8080","--max-connections","512"]
