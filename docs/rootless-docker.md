# Set up a rootless Docker host for Boxes

This runbook sets a Linux host up so `moor up` runs Docker Boxes on rootless Docker.
A Box that breaks out of rootless Docker is your user on this host, not root.
[Rootless Docker Boxes](specs/rootless-docker-boxes.md) gives the design and every check `moor up` runs.
Every host gets the same install, and every Box gets KVM.

You need sudo for the packages, the subordinate ids and the KVM rule. Everything else
runs as the user that will own the Boxes.

## 1. Install rootless Docker

Install Docker's rootless extras, `uidmap`, `slirp4netns` and `util-linux` (for
`nsenter`) from your distribution. Give the user 65536 subordinate ids in both
`/etc/subuid` and `/etc/subgid`.

```sh
sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 "$USER"
```

Where the sysctl `kernel.apparmor_restrict_unprivileged_userns` exists, set it to 0.

Then, as the user:

```sh
dockerd-rootless-setuptool.sh install
systemctl --user enable --now docker
sudo loginctl enable-linger "$USER"
```

## 2. Configure the daemon

Boxes need slirp4netns and a closed host loopback. Set both in the daemon's user unit.

```sh
systemctl --user edit docker
```

```ini
[Service]
Environment=DOCKERD_ROOTLESS_ROOTLESSKIT_NET=slirp4netns
```

- Leave `DOCKERD_ROOTLESS_ROOTLESSKIT_DISABLE_HOST_LOOPBACK` unset. rootlesskit then runs
  with `--disable-host-loopback`, and no Box reaches a `127.0.0.1` listener of this host.
  `moor up` fails when the flag is missing.
- Leave rootlesskit's `--detach-netns` at its default.

Turn live-restore off in the user's Docker daemon.json, under ~/.config/docker. A rootless daemon restart makes
a new network namespace, so live-restore cannot adopt old containers. `moor wake` then
fails with "container with given ID already exists".

```json
{ "live-restore": false }
```

Restart the daemon with `systemctl --user restart docker`.

## KVM

Every Box gets KVM, so `moor up` refuses a host whose Boxes cannot open `/dev/kvm`.
This section is the only KVM step. Replace it whole when the KVM setup changes.

Rootless Docker runs containers with runc, which drops supplementary groups. A Box
therefore cannot use the host's `kvm` group, and `/dev/kvm` must be open to every user.
That also lets every user of the host open `/dev/kvm`.

```sh
echo 'KERNEL=="kvm", MODE="0666"' | sudo tee /etc/udev/rules.d/65-kvm.rules
sudo udevadm control --reload
sudo udevadm trigger --name-match=kvm
ls -l /dev/kvm
```

`ls` shows `crw-rw-rw-`. A missing `/dev/kvm` means virtualization is off in the
firmware, or neither `kvm_intel` nor `kvm_amd` is loaded.

## 3. Run moor up

Point `docker` at the rootless daemon in the shell that runs `moor up`. `moor up` keeps
`DOCKER_HOST` in the machine agent's unit. Leave the docker context at `default`.

```sh
export DOCKER_HOST=unix://$XDG_RUNTIME_DIR/docker.sock
moor up
```

`moor up` writes these user units under `~/.config/systemd/user`.

| Unit | Runs |
| --- | --- |
| `moor.socket`, `moor.service` | The controller |
| `moor-gate.service` | The gate Boxes report steps to, unless you run your own |
| `moor-runner.service` | The machine runner |
| `moor-agent.service` | The machine agent that makes Docker Boxes |
| `moor-relay.service` | The egress relay inside the rootless daemon's network namespace |

The first Box pulls the Box images again. The rootless daemon keeps its own image store.

## 4. Verify

```sh
moor doctor
```

Every line says `ok`, `kvm` and `egress` among them. A `fail` line names its fix.

## Go back to rootful Docker

```sh
moor down
systemctl --user disable --now docker
unset DOCKER_HOST
moor up
```

`moor up` on a rootful daemon removes `moor-relay.service`. Rootful Boxes open
`/dev/kvm` through its group, so the KVM rule can stay.
