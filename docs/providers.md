# Box providers and the Proxy

moor gets each Box's machine from crabbox. The
keys are in the [configuration reference](config.md).

## Use a `-moor` build of crabbox

moor needs crabbox 0.79.1-moor.6 or a newer `-moor` build of
[IniZio/crabbox](https://github.com/IniZio/crabbox). It refuses an older
build and any upstream crabbox release, because only the `-moor` builds have
the flags that moor uses.

## Where crabbox runs

moor runs every crabbox call for a Box from that Box's lease directory,
`<state dir>/leases/<slug>`, which only moor's user can read. crabbox claims a
Box for the directory it runs from, so the claim never names your checkout.
moor sets `GIT_CEILING_DIRECTORIES` to `<state dir>/leases`, so a state
directory inside a git repo does not leak that repo into the claim. moor
removes the directory once crabbox has deleted the Box or reports it gone.

Commands, single-file reads and writes, and copies out of a Box do not go
through crabbox.
moor runs them over one ssh connection per Box, opened with the ssh options
`crabbox ssh` prints for the lease, so host-key checking stays on, as in
crabbox's own: the lease's `known_hosts` file and `StrictHostKeyChecking` set to
`yes` or `accept-new`.
Before any ssh to a Box, including the agent pane's, moor runs `ssh -G` with
those options. `ssh -G` prints the options ssh would use, after it reads every
flag and config file, and it opens no connection. moor refuses the options
unless `StrictHostKeyChecking` comes out as `yes` or `accept-new` and no `UserKnownHostsFile`
comes out as `/dev/null`, another device, `none` or empty. With `accept-new`, moor also refuses a first known-hosts file that ssh cannot add to: one that is not writable, or a missing one in a directory that is not writable. moor also refuses
them when `ssh -G` fails. The command then fails
with an error that says so; moor never falls back to an unchecked connection.
A command's environment travels on ssh's stdin, never in its arguments.
moor puts Box work in the login user's home. It runs every crabbox call
with `CRABBOX_EXTERNAL_WORK_ROOT=moor`. Only the external provider
(agent37, cloud-hypervisor) reads it. It overrides any `external.workRoot` in
your crabbox config, and crabbox resolves it against the home of the user
the lease logs in as: `/root/moor` for a root login, `/home/crabbox/moor`
for a `crabbox` one. You do not need to set `external.workRoot` for the
external provider. Other providers, such as the static one, keep crabbox's
own work root.
A Box's working directory is the one crabbox reports when it leases the Box,
or `<workroot>/<lease id>/<slug>`; when crabbox reports neither, the start
fails. moor refuses a relative work root in that report.

When a release fails, the directory stays and `moor close` names it. To
release the Box by hand, run `crabbox stop --provider PROVIDER --id SLUG
--delete` from that directory, or pass `--reclaim`.

## Choose the provider

To use an external provider, set `box.provider` and cap its live Boxes with
`drivers.external.max_boxes`:

```yaml
box:
  provider: external
drivers:
  external:
    max_boxes: 5
```

Set `max_boxes` to the number of instances that your provider account
allows.

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

Keep agent37's own auto-sleep off. moor sleeps the Box itself, and a Box with
a live tunnel never looks idle to agent37.

## Sleep and wake

moor reads each provider's features from `crabbox providers --json
--configured`. When a provider has the `pause-resume` feature, moor puts a
Box to sleep after it has been idle for `box.sleep_after`. Sleep pauses the instance and wake resumes it. A provider with neither
`pause-resume` nor `native-sleep` cannot sleep, so its Boxes stay up.

## Box image requirements

The Box image must have `sh`, `git`, `curl`, `tar`, and `unzip`. moor's
scripts run under `sh` and use `git` for the checkout.

When the repo has `setup` commands, the setup script checks for these tools
first. For each tool that is missing, it prints a warning that names the
tool, writes the warning to `.moor/setup.log` in the Box, and continues. A missing tool does
not fail the setup. A repo with no `setup` commands runs no setup script, so
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

The socket permissions do not protect a Box that shares the host's network, such
as a crabbox `local-container` Box with `network: host`. On such a Box the
Proxy port is on the host's loopback. Any process on the host, including
other users' processes and other Boxes, can send requests through that Box's
Proxy and get the credentials it adds. moor does not refuse such a Box. Keep
the `bridge` network unless you trust everything that runs on the host.

## Which traffic goes through the Proxy

Only the hosts that a Proxy rule serves go through the Proxy. These are the
hosts that need your credentials, such as `api.anthropic.com` and
`github.com`, plus the hosts of your own rules. Every other host, such as a
package registry, is reached direct from the Box.

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

## Docker in the Box

A `docker build` step and a container run in their own network, so they do not
see the proxy variables of the Box shell. moor writes
`proxies.default.httpProxy`, `httpsProxy` and `noProxy` into
`~/.docker/config.json` of the Box user, before each command. Docker passes
them to every build as build arguments and to every container as `HTTP_PROXY`,
`HTTPS_PROXY` and `NO_PROXY`, so the build and the container reach rule hosts
through the Proxy as the Box shell does. This is the client setting that Docker
documents for proxies.

moor merges the three keys into an existing file and keeps every other key,
such as `auths` and `credsStore`, and the file's mode. A new file has mode 0600.
To merge into an existing file, the Box needs `jq`. Without `jq`, or when the
file is not a JSON object, moor keeps the file, prints a warning on stderr and
runs the command.

The address in the file is `172.17.0.1:<port>`, the gateway of Docker's default
bridge. A container on the default bridge or on a Compose network reaches the
Box at that address. A container does not reach a service of another container
through the proxy: `noProxy` holds `localhost`, `127.0.0.1`, `::1` and
`172.16.0.0/12`, but it cannot hold Compose service names. Add the
names of your services to `NO_PROXY` in the Compose file. When you change
Docker's default bridge address with `bip` or its address pools, the address
in the file no longer matches.

To make this address reachable, tinyproxy listens on every IPv4 address of the
Box and answers only these sources: `127.0.0.1` and `172.16.0.0/12`. The
`172.16.0.0/12` range holds the default bridge and the Compose networks.
tinyproxy takes one `Listen` address, and the bridge gateway exists only after
Docker starts, which can be after moor writes the config. A listener on the
gateway address would fail to start before Docker runs. tinyproxy refuses any
other source with a 403. A Box with a public address, such as one on agent37,
therefore accepts the connection but refuses it before it sends a request
anywhere. A host on the Box's own network segment can still reach the proxy,
and with it the Proxy credentials of the Box, when its address is in
`172.16.0.0/12` or when the Box shares the host's network. A container on one of
Docker's last default networks, in `192.168.0.0/20`, is not answered, because a
Box's own network is often inside that range.

moor does not put the Box CA in a build. Docker has no setting for it. The Box
CA certificate is `~/.moor-proxy/ca.pem` in the Box, and `~/.moor-proxy/ca-bundle.pem`
holds the system bundle with it when the system bundle cannot be edited. A
Dockerfile that must trust the Proxy takes the certificate as a build argument,
for example by copying `ca.pem` into the build context.
