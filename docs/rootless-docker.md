# Set up a rootless Docker host for Boxes

moor runs Docker Boxes only on rootless Docker. A Box that breaks out of rootless Docker is
your user on this host, not root. [Rootless Docker Boxes](specs/rootless-docker-boxes.md)
gives the design and every check.

## One command

On a Linux host with Docker installed the usual way, as root, run as the user that will own
the Boxes:

```sh
curl -fsSL https://raw.githubusercontent.com/IniZio/moor/develop/install.sh | sh -s -- --nightly --server https://moor.example.com
```

The portal's **Add a machine** dialog shows this line with a one-use `--token`, so the
machine pairs with no browser step. Without a token, `up` prints a link to confirm in a
browser. `--name` names the machine.

The script installs `moor-machine-agent` and runs `moor-machine-agent up`. `up` makes the
host ready, pairs it and waits until it links. Each step prints one line, and a step that is
already done prints `ok`. Run it again and nothing changes. With the agent installed, run
`moor-machine-agent up --server https://moor.example.com` instead.

## What it does

| Step | Runs as | What it runs |
| --- | --- | --- |
| Check the host | you | Reads every gap below at once |
| Open `/dev/kvm` to every user | sudo | A udev rule `KERNEL=="kvm", MODE="0666"` in `/etc/udev/rules.d/65-kvm.rules`, then `udevadm` reload, trigger and settle |
| Install packages | sudo | `uidmap`, `slirp4netns`, and `docker-ce-rootless-extras` when Docker's apt or dnf repository is set up |
| Give subordinate ids | sudo | `usermod --add-subuids --add-subgids` with 65536 ids no one else has |
| Allow user namespaces | sudo | `kernel.apparmor_restrict_unprivileged_userns=0`, kept in `/etc/sysctl.d/60-rootless-docker.conf` |
| Linger | you, else sudo | `loginctl enable-linger` |
| Rootless daemon | you | Writes the daemon's settings, runs Docker's `dockerd-rootless-setuptool.sh install --force`, puts the `docker` context back, and runs `systemctl --user enable --now docker` |
| Agent service | you | Writes the agent's user unit with `DOCKER_HOST=unix://$XDG_RUNTIME_DIR/docker.sock` |
| Preflight, pair, link | you | The checks in the spec, then pairing |

Every sudo step goes in one `sudo sh -ec '…'` command. `up` lists the gaps in plain
words and shows the command. Then it asks one question, `Run these with sudo now? [Y/n]`.
Sudo asks for your password once. With no terminal, or on `n`, `up` prints the command and exits 1.
It never runs sudo without asking.

A missing `/dev/kvm` is not a sudo fix. Turn on virtualization in the firmware, and load
`kvm_intel` or `kvm_amd`.

`up` leaves the system Docker daemon running and untouched. Docker's setup tool switches the
`docker` CLI to its `rootless` context, and `up` switches it back to the one you had.
Without Docker's package or repository, `up` runs Docker's own rootless installer,
`https://get.docker.com/rootless`, which puts Docker under `~/bin`. It needs `curl`, `uidmap`,
`iptables` and the subordinate ids, and `up` gives it `FORCE_ROOTLESS_INSTALL=1` while the
system daemon runs.

## By hand

These are the steps `up` runs, for a host where you want to do them yourself.

```sh
sudo apt-get install -y uidmap slirp4netns docker-ce-rootless-extras
sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 "$USER"
echo kernel.apparmor_restrict_unprivileged_userns=0 | sudo tee /etc/sysctl.d/60-rootless-docker.conf
sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
loginctl enable-linger || sudo loginctl enable-linger "$USER"
```

Write the daemon's settings before its first start. Keep anything else your daemon.json says.

```sh
mkdir -p ~/.config/systemd/user/docker.service.d ~/.config/docker
printf '[Service]\nEnvironment=DOCKERD_ROOTLESS_ROOTLESSKIT_NET=slirp4netns\nEnvironment=PATH=/usr/bin:/usr/sbin:/bin:/sbin\n' \
  > ~/.config/systemd/user/docker.service.d/moor.conf
echo '{ "live-restore": false }' > ~/.config/docker/daemon.json
dockerd-rootless-setuptool.sh install --force
docker context use default
systemctl --user enable --now docker
```

- The `PATH` line makes the daemon run the packaged `/usr/bin/rootlesskit`, not an older one
  earlier on your shell's `PATH`.
- Leave `DOCKERD_ROOTLESS_ROOTLESSKIT_DISABLE_HOST_LOOPBACK` unset, so no Box reaches a
  `127.0.0.1` listener of this host. `up` fails when the flag is missing.
- Live-restore stays off. A rootless daemon restart makes a new network namespace, so
  live-restore cannot adopt old containers, and `moor wake` fails with "container with given ID
  already exists".

## KVM

Every Box gets KVM, so `up` refuses a host whose Boxes cannot open `/dev/kvm`. Rootless
Docker runs containers with runc, which drops supplementary groups. A Box therefore cannot use
the host's `kvm` group, and `/dev/kvm` must be open to every user. That also lets every user
of the host open it.

```sh
echo 'KERNEL=="kvm", MODE="0666"' | sudo tee /etc/udev/rules.d/65-kvm.rules
sudo udevadm control --reload
sudo udevadm trigger --name-match=kvm
ls -l /dev/kvm
```

`ls` shows `crw-rw-rw-`.

## A controller host

`moor up` runs the controller and an agent on one host. It checks the host but does not set it
up. Set the host up by hand as above, then point `docker` at the rootless daemon in the shell
that runs `moor up`. `moor up` keeps `DOCKER_HOST` in the agent's unit.

```sh
export DOCKER_HOST=unix://$XDG_RUNTIME_DIR/docker.sock
moor up
moor doctor
```

The first Box pulls the Box images again. The rootless daemon keeps its own image store.

## Roll back moor

To run Boxes on rootful Docker again, install a moor release from before rootful support
was removed, then run `up` again.

```sh
curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | MOOR_VERSION=<tag> sh
moor up
```
