# Box providers and the Proxy

moor gets each Box's machine from crabbox. The provider keys are in the
[configuration reference](config.md).

## Use a `-moor` build of crabbox

moor needs crabbox 0.79.1-moor.6 or a newer `-moor` build of
[IniZio/crabbox](https://github.com/IniZio/crabbox). It refuses an older
build and any upstream crabbox release, because only the `-moor` builds have
the flags that moor uses.

## Where crabbox runs

moor runs every crabbox call for a Box from that Box's lease directory,
`<state dir>/leases/<slug>`. Only moor's user can read that directory. crabbox
claims a Box for the directory it runs from, so the claim never names your
checkout. moor sets `GIT_CEILING_DIRECTORIES` to `<state dir>/leases`, so a
state directory inside a git repo does not leak that repo into the claim. moor
removes the lease directory after crabbox deletes the Box or reports it gone.

Commands, single-file reads and writes, and copies out of a Box do not go
through crabbox. moor runs them over one ssh connection per Box. It opens the
connection with the ssh options that `crabbox ssh` prints for the lease. Those
options keep host-key checking on, as in crabbox's own ssh. They name the
lease's `known_hosts` file and set `StrictHostKeyChecking` to `yes` or
`accept-new`.

Before any ssh to a Box, including the agent pane's, moor runs `ssh -G` with
those options. `ssh -G` reads every flag and config file, prints the options
ssh would use, and opens no connection. moor refuses the options in these
cases:

- `StrictHostKeyChecking` is not `yes` or `accept-new`.
- A `UserKnownHostsFile` is `/dev/null`, another device, `none`, or empty.
- `StrictHostKeyChecking` is `accept-new` and ssh cannot add to the first
  known-hosts file. The file is not writable, or it is missing and its
  directory is not writable.
- `ssh -G` fails.

The command then fails with an error that says so. moor never falls back to an
unchecked connection. A command's environment travels on ssh's stdin, never in
its arguments.

moor puts Box work in the login user's home. It runs every crabbox call with
`CRABBOX_EXTERNAL_WORK_ROOT=moor`. Only the external provider (agent37,
cloud-hypervisor) reads that variable. It overrides any `external.workRoot` in
your crabbox config. crabbox resolves it against the home of the user the
lease logs in as, which gives `/root/moor` for a root login and
`/home/crabbox/moor` for a `crabbox` login. You do not need to set
`external.workRoot` for the external provider. Other providers, such as the
static one, keep crabbox's own work root.

A Box that keeps its [work tree](config.md#work-tree) works in
`<workroot>/repos/<repo name>`, the same directory in every Box of that repo.
Any other Box works in the directory that crabbox reports when it leases the
Box, or else in `<workroot>/<lease id>/<slug>`. When crabbox reports neither,
the start fails. moor refuses a relative work root in that report. A script
that calls `crabbox run --id` without `--workdir` runs in crabbox's per-lease
directory, not in the work tree.

When a release fails, the directory stays and `moor close` names it. To
release the Box by hand, run `crabbox stop --provider PROVIDER --id SLUG
--delete` from that directory, or pass `--reclaim`.

## Choose the provider

A provider runs Boxes through a node. To use an external provider, declare a
node with that runtime and cap its live Boxes with `max_boxes`:

```yaml
nodes:
  ci:
    runtime: external
    max_boxes: 5
```

Set `max_boxes` to the number of instances that your provider account
allows. See [Nodes](config.md#nodes) for labels and for choosing among several
nodes.

`drivers.<provider>.lookup_timeout_secs` sets how many seconds moor waits for
crabbox to look up a Box's ssh target. The default is 5, or 20 for
`external`, because an external provider's lookup goes through its adapter.
Raise it if commands fail with `box operation timed out`.

## Remove platform variables

`drivers.<provider>.strip_env` lists the environment variables that the
provider's platform puts in the Box, such as platform tokens. moor removes
them from every command it runs on the Box: setup, teardown, `moor run`, and
the agent, before the agent's hooks run.

```yaml
drivers:
  external:
    strip_env: [PLATFORM_TOKEN]
```

The agent also starts without crabbox's own variables: its gate names, every
`NSC_*` name, and `token` in any case. Every other Box variable reaches the
agent, so list each platform secret in `strip_env`.

## Use the agent37 provider

[crabbox-agent37](https://github.com/IniZio/crabbox-agent37) is the external
provider for agent37 VMs. Its README lists the crabbox user config it needs,
including `external.capabilities.pauseResume: true`, and the variables its
platform puts in the Box.

Turn off agent37's own auto-sleep. moor sleeps the Box itself, and a Box with
a live tunnel never looks idle to agent37.

## Sleep and wake

moor reads each provider's features from `crabbox providers --json
--configured`. When a provider has the `pause-resume` feature, moor puts a
Box to sleep after it has been idle for `box.sleep_after`. Sleep pauses the
instance and wake resumes it. A provider with neither `pause-resume` nor
`native-sleep` cannot sleep, so its Boxes stay up.

## Cache volumes

A provider with the `cache-volume` feature in `crabbox providers --json
--configured` keeps [cache volumes](config.md#cache-volumes) across Boxes. On
an external provider, only the trusted crabbox user config can turn the
feature on. A repo config, an environment variable, or a flag cannot:

```yaml
external:
  capabilities: { cacheVolumes: true }
```

moor sends the volumes only on `crabbox warmup`, as a JSON array in
`CRABBOX_CACHE_VOLUMES`. Each entry has these fields:

- `group`, 16 hex characters, one group per scope
- `name`
- `key`, which is `<group>-<name>`
- `paths`, the paths in the Box, where `~/` is the login user's home
- `sizeGB`, a size hint, where 0 means the adapter's default
- `"sealed": true`, when the volume has a seal

A sealed volume's group is the scope's 16 hex characters followed by `x`, so
it has a store of its own. The work tree volume, `worktree` at
`~/moor/repos/<repo name>`, is sealed in a group that ends in `w` instead. As a
result, neither its seal nor Docker's seal can discard the other. An adapter
that has the feature gives each lease its own login home, because the work
tree path is the same in every Box of a repo. The adapter keeps one store per
group, with each name as a subdirectory. The adapter decides where the store
lives, how big it can grow, and when old copies go. moor never asks a provider
to require a volume. Any failure starts the Box cold.

At close, after teardown and before release, moor runs each volume's seal in
the Box. After a seal succeeds, moor writes `.crabbox-sealed` into each of that
volume's paths. When the adapter restores the group, it deletes that file from
every path of a sealed volume. On release, the adapter keeps a sealed group
only when every path of each of its sealed volumes holds the file again. The
adapter keeps a Box's copy on release after a clean shutdown. A Box that is
killed, or only paused, keeps nothing.

When the Box has a repo, moor also sends `CRABBOX_CACHE_SCOPE`. Its value is
the repo group's 16 hex characters, without the `x` of a sealed group. A Box
without a repo, or without volumes, gets no scope.

A provider without the feature starts every Box cold. A start warns
`cache_unavailable` when the repo or the operator declared volumes of their
own. Presets alone start cold without a warning.
`drivers.<provider>.cache_volumes: false` turns the volumes off for one
provider without the warning. Docker's data root is `~/docker` on every
external-provider Box. Docker 29 keeps images in containerd's store at
`/var/lib/containerd`, so the `docker` preset keeps both. An adapter must
mount the cache before `containerd.service` and `docker.service` start.

agent37 leaves `cacheVolumes` off, so its Boxes start cold.

## Box image requirements

The Box image must have `sh`, `git`, `curl`, `tar`, and `unzip`. moor's
scripts run under `sh` and use `git` for the checkout.

When the repo has `setup` commands, the setup script checks for these tools
first. For each missing tool, it prints a warning that names the tool, writes
the warning to `.moor/setup.log` in the Box, and continues. A missing tool
does not fail the setup. A repo with no `setup` commands runs no setup script, so
nothing checks the tools.

After setup, moor copies the Box's `.moor/setup.log` to
`<state dir>/logs/<box id>/setup.log` on the host. Only moor's user can read
that directory and file.

The image's sshd must not set `GatewayPorts yes`. With `yes`, the Box's Proxy
port listens on the Box's network, and other machines on that network can
use the Box's Proxy. The default, `no`, keeps the port on the Box's loopback.

## How a Box reaches its Proxy

Each Box has its own Proxy on the host. The Box reaches it through a reverse
tunnel. The tunnel listens on `127.0.0.1:<port>` inside the Box and carries
each connection to a Unix socket in `/tmp/moor-<uid>` on the host. Only your
user can open that socket. The Proxy checks no credential from the Box, so
the Box holds no Proxy secret.

The socket permissions do not protect a Box that shares the host's network,
such as a crabbox `local-container` Box with `network: host`. On such a Box the
Proxy port is on the host's loopback. Any process on the host, including
other users' processes and other Boxes, can send requests through that Box's
Proxy and get the credentials it adds. moor does not refuse such a Box. Keep
the `bridge` network unless you trust everything that runs on the host.

## Which traffic goes through the Proxy

Only the hosts that a Proxy rule serves go through the Proxy. These are the
hosts that need your credentials, such as `api.anthropic.com` and
`github.com`, plus the hosts of your own rules. The Box reaches every other
host, such as a package registry, directly.

moor runs tinyproxy on the Box loopback to make this split. Before each
command, moor checks that tinyproxy listens. When it does not, moor starts it
with the config `~/.moor/egress.conf` and the pid file `~/.moor/egress.pid`.
The config sends each rule host to the Proxy tunnel and lets tinyproxy dial
every other host. `HTTPS_PROXY`, `HTTP_PROXY`, their lowercase forms, the Yarn
variables and the Java and Gradle proxy settings name the tinyproxy port.
`NO_PROXY` starts with the loopback names and keeps any value the Box already
sets after them.

Each Box keeps one tinyproxy port for its life. moor picks it from 10000 to
19999 when it first starts the Box's Proxy, and no two Boxes share it. When
another process in the Box already listens on that port, the command stops
with `port <n> is taken by another process`. When two commands start at once,
only one of them installs and starts tinyproxy. The other waits for it.

When tinyproxy is not installed, moor installs the `tinyproxy-bin` package
with `apt-get`, as root or through `sudo -n`, and without the proxy
variables. moor supports only apt. On a Box image with no `apt-get`, install
`tinyproxy` in the image. When moor cannot install or start tinyproxy, the
command stops before it runs, and the reason starts with `the egress proxy
could not start`. A Box setup that stops this way fails with that reason.
moor never sends all Box traffic through the Proxy instead.
