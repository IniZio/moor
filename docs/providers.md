# Box providers and the Proxy

moor gets each Box's machine from crabbox. The
keys are in the [configuration reference](config.md).

## Use a `-moor` build of crabbox

moor needs crabbox 0.79.1-moor.1 or a newer `-moor` build of
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

## Direct hosts

`proxy.direct` lists hosts that the Box reaches without the Proxy, such as
public package mirrors. Both `.moor.yaml` and your user config can set it.
moor uses your entries first, then each repo entry that is not already
listed.

```yaml
proxy:
  direct: [dl.google.com, repo.maven.apache.org, .gradle.org, pub.dev]
```

An entry is a host name, or `.domain` for every name under the domain. moor
lowercases each entry and adds it to `NO_PROXY` and `no_proxy`, after the
loopback entries `127.0.0.1,localhost,::1`. In the Java and Gradle
`nonProxyHosts` that moor writes, `.domain` becomes `*.domain`.

A host that a credential rule serves can never go direct. moor refuses an
entry that covers one, and the error names the entry and the host. An entry
covers its own name and every name under it, in either form, because curl
and Go read a bare `NO_PROXY` name that way. For example, moor refuses `.github.com`,
`anthropic.com`, and `api.github.com`.
