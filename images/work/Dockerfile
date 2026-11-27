# syntax=docker/dockerfile:1.7
# The work place of a Box: where repos build, services run and Docker runs inside. It is
# privileged and joins the agent place's network namespace. Everything it needs from moor
# (moor-work-init, box-rebuild, the ssh key, the env) arrives on the /run/moor volume that the
# agent place fills, so any image that has bash and sshd can stand in for this one.

FROM ubuntu:24.04@sha256:73b999393d07f2bf65a40373778a44dc9a6d54ad7718d29247d8ff9413c4d6a8

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      bash ca-certificates curl git jq less sudo tini unzip xz-utils rsync procps iproute2 \
      build-essential openssh-server openssh-client \
      docker.io docker-compose-v2 docker-buildx \
 && rm -rf /var/lib/apt/lists/* \
 # The image's own uid 1000 makes way for the Box user, whose files the agent place shares.
 && (userdel -r ubuntu 2>/dev/null || true) \
 && groupadd --gid 1000 agent \
 && useradd --uid 1000 --gid 1000 --create-home --shell /bin/bash --password '*' agent \
 && usermod -aG docker agent \
 && echo 'agent ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/agent \
 && chmod 0440 /etc/sudoers.d/agent \
 && mkdir -p /run/sshd /home/agent/moor \
 && chown agent:agent /home/agent/moor

ARG GH_VERSION=2.105.0
ARG GH_SHA256_AMD64=79e7ab5db738c48777b696e374f8b428f0647debfb424f44d2f426370407fe2f
ARG GH_SHA256_ARM64=b1278c75107423d818f1937fd92cafc9e5e9b606631568f68d7fb816f5885a02

RUN case "$(uname -m)" in \
      x86_64) arch=amd64; sha=${GH_SHA256_AMD64} ;; \
      aarch64) arch=arm64; sha=${GH_SHA256_ARM64} ;; \
      *) echo "gh ${GH_VERSION} has no build for $(uname -m)" >&2; exit 1 ;; \
    esac \
 && curl -fsSL --retry 5 --retry-all-errors --connect-timeout 20 \
      "https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_${arch}.tar.gz" -o /tmp/gh.tar.gz \
 && echo "${sha}  /tmp/gh.tar.gz" | sha256sum -c - \
 && tar -xzf /tmp/gh.tar.gz --no-same-owner -C /usr/local/bin --strip-components=2 "gh_${GH_VERSION}_linux_${arch}/bin/gh" \
 && rm /tmp/gh.tar.gz \
 && gh --version

# Present only when the volume that holds it is mounted: the provider mounts /run/moor.
ENTRYPOINT ["/run/moor/bin/moor-work-init"]
