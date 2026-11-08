# Run moor for a team

This page is for an operator. It takes one Linux host from nothing to an MCP
URL that teammates connect to. You need an OIDC sign-in provider, such as
Authgear, and a DNS name for the host, written here as `<host>`, with TLS in
front of it.

## 1. Install and set the host up

```sh
curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | sh
moor up
```

Boxes get their machines from crabbox, so the host also needs a `-moor`
build of crabbox on `PATH`, as [Box providers](providers.md#use-a--moor-build-of-crabbox)
says. `moor up` prints one line per step, with a fix for any that fails, and a
rerun changes nothing:

- It checks that crabbox selects a provider and that `crabbox doctor` passes
  for it. When crabbox selects none, it prints the commands that install the
  cloud-hypervisor provider.
- It writes the controller config, `~/.config/moor/controller.yaml`, and a
  `local` node on that provider in `~/.config/moor/config.yaml`. A
  `--max-boxes N` flag caps the node. [Nodes](nodes.md) covers labels and
  memory admission.
- It runs the controller as the systemd user units `moor.socket` and
  `moor.service`, which [Deploy the controller with systemd](deploy.md)
  describes. Run `loginctl enable-linger` once so they keep running after you
  log out.
- It creates your moor user and asks for the token that `claude setup-token`
  prints and the one that `gh auth token` prints. To skip the prompts, set
  `CLAUDE_CODE_OAUTH_TOKEN` and `GH_TOKEN`.
- It ends with `moor doctor`.

Back up `vault.key` from `~/.config/moor`. Without it, no stored secret can be
read. `moor down` stops and removes the units, and keeps the config, state and
secrets.

## 2. Start a Box

```sh
moor start hello --repo OWNER/NAME --prompt "Say hello and stop"
```

`moor start` refuses with every missing secret named, if one is missing.
Close the Box with `moor close hello` once it is ready.

## 3. Add the gate's front

The gate talks to the controller through the `authgear` front. Add it to
`~/.config/moor/controller.yaml`, add its socket unit as
[Deploy the controller with systemd](deploy.md#install-the-units) shows, and
restart the controller:

```yaml
fronts:
  authgear: {}
```

```sh
systemctl --user enable --now moor-front@authgear.socket
systemctl --user restart moor.service
```

## 4. Start the gate

The gate checks each person's sign-in and serves the MCP endpoint. Register
`https://<host>/auth/callback` as a redirect URI on the gate's client at your
provider, and have the provider issue access tokens for `https://<host>/mcp`
with the `moor:read` and `moor:write` scopes. Then:

```sh
moor-gate \
  --front-socket ~/.local/state/moor/fronts/authgear/front \
  --issuer https://<your provider> \
  --client-id <client id> \
  --mcp-resource-url https://<host>/mcp \
  --public-url https://<host>
```

The gate listens on `0.0.0.0:8443` without TLS. Put your TLS ingress in
front of it, and let it hold a response stream open. The details are in
[Operator setup](mcp.md#operator-setup) and
[Browser sign-in](mcp.md#browser-sign-in).

## 5. Hand out the URL

Give teammates `https://<host>/mcp` and [Use moor on your team](use.md).
Each one signs in as themselves and links their own tokens. To give them a
place to link tokens, run the [Slack front](slack.md) or point them at the
[herdr plugin](herdr.md#a-remote-moor).

## On Kubernetes

The release image, `ghcr.io/inizio/moor`, holds the controller, the gate,
the Slack front and crabbox. The Helm chart runs the
controller and the gate from it; [Run moor on Kubernetes](kubernetes.md)
installs it. The controller config, the node, and the
gate's flags are the same as above. Read [Deploy the controller with systemd](deploy.md) for how the
controller drains on a restart, which the chart's stop timeout must allow
for.

## Reference

- [Configuration reference](config.md): every key of the user config,
  `.moor.yaml`, and the controller config.
- [Nodes](nodes.md), [providers and the Proxy](providers.md), and
  [machines](machines.md): where Boxes run.
- [Slack](slack.md) and [webhooks](notify.md): more fronts.
