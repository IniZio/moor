# Run moor for a team

This page is for an operator. It takes one Linux host from nothing to an MCP
URL that teammates connect to. You need an OIDC sign-in provider, such as
Authgear, and a DNS name for the host, written here as `<host>`, with TLS in
front of it.

## 1. Install the binaries

```sh
curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | sh
```

This installs the moor release binaries. Boxes get their
machines from crabbox, so the host also needs a `-moor` build of crabbox on
`PATH`, as [Box providers](providers.md#use-a--moor-build-of-crabbox) says.

## 2. Start the controller

The controller owns the state, the vault of secrets, and every Box. Keep its
config outside the state directory, because it keeps its vault key beside the
config. The `authgear` front is the socket the gate talks through:

```sh
mkdir -p ~/.config/moor-controller
cat > ~/.config/moor-controller/config.yaml <<'EOF'
socket: ~/.local/state/moor/controller.sock
fronts:
  authgear: {}
EOF
moor controller serve --config ~/.config/moor-controller/config.yaml
```

Back up `vault.key` from that directory. Without it, no stored secret can be
read. For a service that survives restarts, use the systemd units in
[Deploy the controller with systemd](deploy.md).

## 3. Give it a node

A node is a place Boxes run. moor has no default, and a start refuses until
one is set. Name a shared node in the user config of the account that runs
the controller, `~/.config/moor/config.yaml`:

```yaml
nodes:
  local:
    runtime: cloudhypervisor
    max_boxes: 4
```

`runtime` is the crabbox provider. [Nodes](nodes.md) covers labels, memory
admission, and personal nodes that a teammate enrolls from their own machine.

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

## 5. Enroll yourself and check a start

Point the CLI at the controller, create your user, and store your tokens.
Each command reads the value from stdin:

```sh
export MOOR_CONTROLLER=~/.local/state/moor/controller.sock
moor user enroll
claude setup-token   # copy the token it prints
printf '%s' '<the Claude token>' | moor user secret set claude
gh auth token | moor user secret set github
moor doctor
moor start smoke --repo OWNER/NAME --prompt "Say hello and stop"
```

`moor doctor` prints one line per check (controller, provider, node, user,
secrets, surface) with a fix for each that fails.
`moor start` refuses with every missing secret named, if one is missing.
Close the Box with `moor close smoke` once it is ready.

## 6. Hand out the URL

Give teammates `https://<host>/mcp` and [Use moor on your team](use.md).
Each one signs in as themselves and links their own tokens. To give them a
place to link tokens, run the [Slack front](slack.md) or point them at the
[herdr plugin](herdr.md#a-remote-moor).

## On Kubernetes

The release image, `ghcr.io/inizio/moor`, holds the controller, the gate,
the Slack front, crabbox, `herdr` and `gh`. The Helm chart runs the
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
