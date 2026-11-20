# Choose an install

moor runs as one controller with a gate in front of it. Choose where the
controller runs, then install the binaries that the choice needs.

| You have | Install | Guide |
| --- | --- | --- |
| One Linux host with Docker, for yourself or a small team | The release binaries, run as systemd user units | [Self-host on one machine](run.md), then [Run on one host](deploy.md) |
| A Kubernetes cluster | The Helm chart, which runs the release image | [Kubernetes](kubernetes.md) |

Either way, Boxes run on nodes. A node is a machine whose agent makes Docker
Boxes. On one host, `moor up` makes the host itself a node. On Kubernetes, the chart's
`controller.nodes` declares shared nodes on a cloud provider, and teammates add
their own machines as personal nodes.

## Install the release binaries

```sh
curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | sh
```

The script downloads the release for your OS and CPU, checks its SHA-256, and
installs into `~/.local/bin`. Releases cover Linux on x86_64 and aarch64, and
macOS on aarch64 (Apple Silicon). There is no Intel Mac build. On one the script
fails and prints this message:

```text
moor has no Intel Mac build; use an Apple Silicon Mac or Linux
```

Without arguments it installs `moor`, `moor-gate`, `moor-egress-forward`,
`moor-machine-agent`, and `moor-machine-runner`.
Name binaries to install others instead:

```sh
curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | sh -s moor moor-gate moor-listen
```

| Binary | Who runs it |
| --- | --- |
| `moor` | The operator. It is the controller, the dev CLI, and the local client for `moor login` and `moor listen`. |
| `moor-gate` | The operator, for teammates to reach moor over HTTPS. |
| `moor-egress-forward` | Nobody by hand. The controller delivers it into each Box. |
| `moor-machine-agent` | Each person who adds a machine. `moor up` runs it for the host's own node. |
| `moor-machine-runner` | The operator. `moor up` runs it beside the controller, to take agent links. |
| `moor-listen` | Anyone who wants event lines with a token of their own. |

| Option | Effect |
| --- | --- |
| `--nightly` or `MOOR_CHANNEL=nightly` | Take the newest nightly release. |
| `MOOR_VERSION=TAG` | Take one release, such as `v0.3.0`. |
| `MOOR_INSTALL_DIR=DIR` | Install into `DIR`. |

## Use the release image

`ghcr.io/inizio/moor` holds the controller, the gate, the webhook
front, and the machine runner. The Helm chart at
`oci://ghcr.io/inizio/charts/moor` runs it. Each release publishes the chart
and the image at the same version.

## Pick a channel

Stable releases come from `main`, with tags like `v0.3.0`. Nightly releases
come from `develop`, with tags like `v0.3.0-nightly.76`. Keep the controller,
the gate, and every machine agent on the same channel.
