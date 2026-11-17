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
- `gate.login` turns on browser sign-in, which the account page, webhook
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

The listener keeps each Box's MCP session in memory. With `gate.separate` and
two or more replicas, a Box's calls can reach a replica without its session.
Keep one gate replica until the listener shares sessions.

## Run the controller without a volume

With `database.url` and `blobs.s3` both set, the controller keeps its store in
Postgres and its artifacts in a bucket. The pod then mounts no volume and rolls
like any Deployment. This needs `egress.enabled`, and the chart refuses to
render otherwise.

To move existing artifacts into the bucket, set `blobs.copyFromVolume: true`
for one deploy. [Helm values](helm.md#blobs) has the steps.
