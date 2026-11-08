# Choose an install

moor runs as one controller with a gate in front of it. Choose where the
controller runs, then install the binaries that the choice needs.

| You have | Install | Guide |
| --- | --- | --- |
| One Linux host with KVM, for yourself or a small team | The release binaries, run as systemd user units | [Self-host on one machine](run.md), then [Run on one host](deploy.md) |
| A Kubernetes cluster | The Helm chart, which runs the release image | [Kubernetes](kubernetes.md) |

Either way, Boxes run on nodes. A node is a crabbox provider on some host. On
one host, `moor up` makes the host itself a node. On Kubernetes, the chart's
`controller.nodes` declares shared nodes on a cloud provider, and teammates add
their own machines as personal nodes.

## Install the release binaries

```sh
curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | sh
```

The script downloads the release for your OS and CPU, checks its SHA-256, and
installs into `~/.local/bin`. Releases cover Linux and macOS on x86_64 and
aarch64.

Without arguments it installs `moor`, `moor-gate`, and `moor-egress-forward`.
Name binaries to install others instead:

```sh
curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | sh -s moor moor-gate moor-slack
```

| Binary | Who runs it |
| --- | --- |
| `moor` | The operator. It is the controller, the dev CLI, and the local client for `moor login` and `moor listen`. |
| `moor-gate` | The operator, for teammates to reach moor over HTTPS. |
| `moor-egress-forward` | Nobody by hand. The controller delivers it into each Box. |
| `moor-slack` | The operator, for the Slack front. |
| `moor-machine-agent` | Each person who adds a machine. |
| `moor-listen` | Anyone who wants event lines with a token of their own. |
| `moor-herdr` | Installed by the herdr plugin, not by hand. |

| Option | Effect |
| --- | --- |
| `--nightly` or `MOOR_CHANNEL=nightly` | Take the newest nightly release. |
| `MOOR_VERSION=TAG` | Take one release, such as `v0.3.0`. |
| `MOOR_INSTALL_DIR=DIR` | Install into `DIR`. |

## Use the release image

`ghcr.io/inizio/moor` holds the controller, the gate, the Slack front, the
webhook front, the machine runner, and crabbox. The Helm chart at
`oci://ghcr.io/inizio/charts/moor` runs it. Each release publishes the chart
and the image at the same version.

## Pick a channel

Stable releases come from `main`, with tags like `v0.3.0`. Nightly releases
come from `develop`, with tags like `v0.3.0-nightly.76`. Keep the controller,
the gate, and every machine agent on the same channel.
