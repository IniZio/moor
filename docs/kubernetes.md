# Kubernetes

Install moor on a Kubernetes cluster with its Helm chart. The chart runs the
controller and the gate in one pod by default, from the release image of the
same version. State lives on one PersistentVolumeClaim, `moor-state`, which
the chart keeps when you uninstall the release.

## Check the cluster

- Pods with user namespaces (`hostUsers: false`). The `UserNamespacesSupport`
  feature is on by default since Kubernetes 1.33.
- A namespace whose Pod Security level admits seccomp `Unconfined`, such as
  `privileged`. The front sandbox needs `pivot_root`.
- A StorageClass for `moor-state`.
- An ingress class that keeps WebSocket upgrades open for hours.

## Fill the required values

The chart refuses to render while any of these is empty, and names the key.

| Key | Value |
| --- | --- |
| `host` | The gate's public hostname. |
| `authgear.issuer` | Your OIDC issuer. See [Sign-in with OIDC](oidc.md). |
| `authgear.clientId` | The OIDC client people sign in with. |
| `mcp.resourceUrl` | `https://HOST/mcp`, registered at the issuer with the scopes `moor:read` and `moor:write`. |
| `gate.ingress.className` | The ingress class for `host`, when `gate.ingress.enabled`. |
| `vault.key` | The `AGE-SECRET-KEY-` line that `age-keygen` prints. One per install. |

Keep `vault.key` in a values file you encrypt, or pass it from a secret store.
The chart puts it in the Secret `<release>-secrets`.

## Install

```sh
helm install moor oci://ghcr.io/inizio/charts/moor --version VERSION \
  --namespace moor -f values.yaml -f secrets.yaml
```

Releases are on the [releases page](https://github.com/IniZio/moor/releases).
Nightly versions look like `0.3.0-nightly.76`.

## Turn on what you need

- `gate.ingress.enabled` exposes the gate on `host`.
- `gate.login` turns on browser sign-in, which the portal, webhook
  setup, and machine pairing need. Register `https://HOST/auth/callback`
  first.
- `notify.enabled` runs the webhook front, so [webhooks](notify.md#at-a-webhook)
  get delivered.
- `runner.enabled`, on by default, lets teammates
  [add their machines](add-machine.md). Set it to `false` to take none.

[Helm values](helm.md) describes every block.

## Keep Boxes up through upgrades

By default each upgrade replaces the one pod. Boxes lose their egress, and
people lose `/mcp`, until the new pod is up. Move the parts Boxes depend on
out of the controller pod:

```yaml
egress:
  enabled: true
gate:
  separate: true
runner:
  separate: true
```

- `egress.enabled` runs the shared egress Proxy as two replicas. Boxes keep
  reaching GitHub and Anthropic while the controller restarts.
- `gate.separate` runs the gate as its own replicas, which hold `/mcp` for up
  to 30 s while the controller is away.
- `runner.separate` runs the machine runner as its own replicas, so paired
  machines stay linked.

`podDisruptionBudget.enabled` adds a budget for the gate and runner replicas.
It needs RBAC on `policy/v1`, which some installs do not grant.

## Keep machine links on your private network

A paired machine holds one WebSocket to `/machines/link` for as long as it is
up, and every terminal keystroke to it crosses that link. When your machines
and the cluster share a network, send the link over that network instead of out
through the public ingress and back. A tunnel or CDN in front of `host` adds
its round trip to each crossing. This is optional and off by default.

```yaml
gate:
  linkIngress:
    enabled: true
    className: internal
    host: LINK_HOST
    tls:
      secretName: link-tls
```

`LINK_HOST` is a name your machines resolve to this ingress, such as through
your private DNS. The chart renders a second Ingress, `<release>-gate-link`.
It routes only `/machines/link` and `/machines/pair`, to the same Services as
the main ingress. The sign-in, portal, MCP and egress paths are not on it. It
needs `runner.enabled`. `className` empty uses the cluster's default class.
`tls.secretName` empty serves `LINK_HOST` without TLS. The link carries
terminal traffic, so give it TLS unless you trust every hop of the network.

The agent checks a certificate against the public web roots only. It does not
read the system store, so a certificate from a private CA fails the link. Use a
public CA, such as an ACME DNS-01 certificate for a name in your own domain
whose private DNS record points at the ingress. Or serve `http://` on a network
you trust.

### Hold the link open

The link is a WebSocket that lasts hours, and the agent sends a keepalive every
5 s. Check these on the ingress that serves `LINK_HOST`:

- No auth middleware on this host. Agents prove themselves with their signed
  key at the handshake, and they carry no cookie or token for a proxy to check.
  A Traefik `forwardAuth` middleware, an nginx `auth-url` annotation or
  `global-auth-url`, or an oauth2-proxy in front would answer every link with a
  redirect.
- Timeouts that outlast the session. For ingress-nginx, set
  `nginx.ingress.kubernetes.io/proxy-read-timeout: "3600"` and
  `proxy-send-timeout: "3600"` in `gate.linkIngress.annotations`, and add
  `nginx.ingress.kubernetes.io/enable-global-auth: "false"` when a global auth
  is set. nginx upgrades WebSockets without further settings. Traefik sets
  its timeouts on the entry point, not the Ingress. Raise `readTimeout`,
  `writeTimeout` and `idleTimeout` under `transport.respondingTimeouts`. A
  value of 0 turns one off. Keep any default middleware off that entry point.
- No cap on a connection's age, such as a maximum lifetime on a load balancer
  or a service mesh. The agent dials again within seconds when a cap cuts the
  link, so a cap shows as a link that drops every N minutes.

Test it by leaving a machine linked past your longest timeout and checking that
its `linked` line in the agent's log has not changed.

### Point a machine at the link host

The agent dials the server in its `state.json`, and pairing and the link use
that one address. The link host takes both, since the Ingress carries
`/machines/pair` too. The browser step of a new pairing still opens on `host`.

To move a machine that is already paired, without pairing again, edit
`server` in `~/.local/state/moor-machine-agent/state.json` to
`https://LINK_HOST` (`http://` when you left TLS off). Then restart the
service with `systemctl --user restart moor-machine-agent`, or the launch
agent on macOS. The machine id and key stay, and the controller sees the same
machine. The service reads `state.json` and holds no server address of its own.

Do not run `moor-machine-agent up --server https://HOST` on that machine
afterwards. `up` compares the address with the one in `state.json` and
pairs again when they differ. To pair a new machine on the link host, run
`up --server https://LINK_HOST` and confirm the code as usual. The agent
keeps one address for both, so the public portal needs no change.

## Step reports

`gate.stepReports: true` lets the agent in a Box report its workflow steps. The
Proxy routes `moor.moor.internal` to the gate's Box listener and stamps the
Box's id, so the Box holds no token. The chart then gives every Box the `moor`
MCP server and lets the gate's controller entry report steps.

With `egress.enabled` the gate binds the listener on its pod address, and the
egress pods reach it through the ClusterIP Service `<release>-gate-boxes`. The
listener trusts the Box id it is sent, and the chart has no NetworkPolicy. So
turn the key on only where nothing but the egress pods can reach
`gate.boxPort` on the gate pods. A NetworkPolicy of your own can admit that
port only from pods labelled `app.kubernetes.io/name: moor-egress`. It must
keep the pod's other ports open.

`gate.boxTools: true` lets the gate's Box listener run `exec_in_box`, `exec_wait`, `start_box` and `close_box` for the agent in a claimed place. It needs `gate.stepReports`.

With `gate.login` on, the controller's `portal_url` is `https://<host>`, so a run's Box gets its run page in `MOOR_RUN_URL`.

`controller.agentPlaces.users` lists the emails of the users who get an agent place. Empty starts none. See [`agent_places`](config.md#agent_places).

## Run the controller without a volume

With `database.url` and `blobs.s3` both set, the controller keeps its store in
Postgres and its artifacts in a bucket. The pod then mounts no volume and rolls
like any Deployment. This needs `egress.enabled`, and the chart refuses to
render otherwise.

To move existing artifacts into the bucket, set `blobs.copyFromVolume: true`
for one deploy. [Helm values](helm.md#blobs) has the steps.
