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

# Present only when the volume that holds it is mounted: the provider mounts /run/moor.
ENTRYPOINT ["/run/moor/bin/moor-work-init"]
