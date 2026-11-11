# Set up the Box provider

moor gets each Box's VM from crabbox, and crabbox gets it from a provider. Set
up crabbox and one provider on every host that runs Boxes. That is the
controller host for a shared node, or a teammate's machine for a personal node.

## Install crabbox

moor needs crabbox 0.79.1-moor.6 or a newer `-moor` build of
[IniZio/crabbox](https://github.com/IniZio/crabbox) on `PATH`. It refuses an
upstream crabbox release, because only the `-moor` builds have the flags moor
uses. Download a `-moor` release from that repo and put `crabbox` on `PATH`.

## Install the cloud-hypervisor provider

[crabbox-cloudhypervisor](https://github.com/IniZio/crabbox-cloudhypervisor)
runs each Box as a cloud-hypervisor VM on the host. crabbox reaches it as its
`external` provider. Your user needs access to `/dev/kvm`.

```sh
curl -fsSL https://github.com/IniZio/crabbox-cloudhypervisor/releases/latest/download/install.sh | sh
crabbox-cloudhypervisor install
```

Then select the provider in the file that `crabbox config path` prints, as the
provider's README shows. When crabbox selects no provider, `moor up` prints
the install commands for the release it was built against.

## Turn on warm starts

Three settings in the crabbox config let moor save a Box's setup and start the
next Box from it:

```yaml
external:
  capabilities: { checkpoints: true }
  config: { checkpoints: true, bootTemplate: true }
```

- `capabilities.checkpoints` lets moor save and fork a Box's VM.
- `config.checkpoints` makes `crabbox doctor` fail unless cloud-hypervisor is
  the patched `v52.0-moor.1`. A stock build loses the guest's network after a
  checkpoint.
- `bootTemplate` starts a new VM from a snapshot of a provisioned guest, in
  about 4 s instead of 41 s. Put the provider's `cacheDir` on a filesystem
  that reflinks, such as btrfs or XFS.

On a personal node, `moor-machine-agent up` adds both `checkpoints` keys when
they are missing. It never changes a value you set, so `false` turns warm
starts off. `moor node ls` shows `warm starts on`, or `off` with the reason.

## Keep caches between Boxes

To keep package caches and the work tree between Boxes, let the provider keep
cache volumes. To also save them while a quiet Box sleeps, add
`cacheSaveOnPause`:

```yaml
external:
  capabilities: { cacheVolumes: true, cacheSaveOnPause: true }
```

Only this trusted crabbox config can turn these on. A repo or a flag cannot.
Without them, every Box starts cold and records a `cache_cold` event. An external
node starts cold unless its crabbox config declares these capabilities.

## Use agent37 instead

[crabbox-agent37](https://github.com/IniZio/crabbox-agent37) runs Boxes on
agent37 cloud VMs. Its README lists the crabbox config it needs, including
`external.capabilities.pauseResume: true`. Turn off agent37's own auto-sleep,
because moor sleeps each Box itself. agent37 keeps no cache volumes, so its
Boxes start cold.

## Declare a shared node

On the controller host, a node in the user config names the provider:

```yaml
nodes:
  ci:
    runtime: external
    max_boxes: 5
```

Set `max_boxes` to the number of VMs your provider account allows.
`drivers.<provider>.strip_env` lists variables the provider puts in the Box,
such as a platform token. moor removes them from every command it runs there.
[Configuration](config.md#nodes) has every node key.

## Check the setup

```sh
crabbox doctor --provider external
moor doctor
```

`moor doctor` prints one `crabbox` line per node runtime and says which check
failed, with its hint.

## Prepare the Box image

The Box image needs `sh`, `git`, `curl`, `tar`, and `unzip`. When `tinyproxy`
is missing, moor installs it with `apt-get`. An image without `apt-get` must
ship `tinyproxy`. Keep the image's sshd at `GatewayPorts no`, so the
Proxy port stays on the Box's loopback.
