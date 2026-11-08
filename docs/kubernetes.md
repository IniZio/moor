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
| `crabbox.agent37Token` | The provider credential, for agent37. |

Keep `vault.key`, the tokens, `crabbox.config`, and `crabbox.sshKey` in a
values file you encrypt, or pass them from a secret store. The chart puts them
in the Secret `<release>-secrets`.

`crabbox.config` is the crabbox config for the shared nodes. For agent37:

```yaml
crabbox:
  config:
    external:
      command: /usr/local/bin/crabbox-agent37
      capabilities: {idempotentLeaseId: true, pauseResume: true, cacheVolumes: true}
      connection:
        ssh:
          trustProviderOutput: true
      config:
        owner: OWNER
        region: REGION
        keyFile: /var/lib/moor/config/agent37.key
        sshKey: /var/lib/moor/config/box-ssh-key
controller:
  nodes:
    cloud: {runtime: external, max_boxes: 5}
```

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
- `slack.enabled` runs the [Slack front](slack.md). Set `slack.appToken` and
  `slack.botToken`.
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
slack:
  separate: true
```

- `egress.enabled` runs the shared egress Proxy as two replicas. Boxes keep
  reaching GitHub and Anthropic while the controller restarts.
- `gate.separate` runs the gate as its own replicas, which hold `/mcp` for up
  to 30 s while the controller is away.
- `runner.separate` runs the machine runner as its own replicas, so paired
  machines stay linked.
- `slack.separate` runs Slack in its own pod, on a new volume. Turning it on
  loses the old threads' context and each channel's repo list.

`podDisruptionBudget.enabled` adds a budget for the gate and runner replicas.
It needs RBAC on `policy/v1`, which some installs do not grant.

## Run the controller without a volume

With `database.url` and `blobs.s3` both set, the controller keeps its store in
Postgres and its artifacts in a bucket. The pod then mounts no volume and rolls
like any Deployment. This needs `egress.enabled` and `slack.separate`, and the
chart refuses to render otherwise.

To move existing artifacts into the bucket, set `blobs.copyFromVolume: true`
for one deploy. [Helm values](helm.md#blobs) has the steps.
