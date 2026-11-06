# Run moor on Kubernetes

The Helm chart runs moor in one pod: the controller, the gate (sign-in,
`/front` and `/mcp`), and, when you turn them on, the Slack front, the
webhook front and the machine runner. State lives on one PersistentVolumeClaim,
`moor-state`, which the chart keeps when you uninstall the release.

Every release publishes the chart next to its image. The chart version is the
release version, and the chart runs the image of that same version unless you
set `image.tag`.

```sh
helm install moor oci://ghcr.io/inizio/charts/moor --version <version> \
  --namespace moor -f values.yaml -f secrets.yaml
```

Releases are listed on the [releases page](https://github.com/IniZio/moor/releases).
Nightly releases have versions like `0.3.0-nightly.76`.

## What the cluster needs

- User namespaces for pods (`hostUsers: false`), the `UserNamespacesSupport`
  feature, on by default since Kubernetes 1.33. The
  pod runs as an unprivileged uid; the front shim needs `pivot_root`, so the
  controller container runs with seccomp `Unconfined` inside its user
  namespace.
- A namespace whose Pod Security level admits that, such as `privileged`.
- A StorageClass for `moor-state` (`persistence.storageClassName`, empty for
  the cluster default).
- An ingress class that keeps WebSocket upgrades open for hours, if people
  reach the gate through an Ingress. `/front` is one WebSocket per session.

## Values you fill

The chart refuses to render while any of these is empty, and names the key.

| Key | What it is |
| --- | --- |
| `host` | The public hostname of the gate. |
| `authgear.issuer` | Your OIDC issuer. Read [Run moor for a team](run.md) for what the gate needs from it. |
| `authgear.clientId` | The native client people sign in with; its redirect URI is `http://localhost:53682/callback`. |
| `mcp.resourceUrl` | `https://<host>/mcp`, registered at the issuer as an API resource with scopes `moor:read` and `moor:write`. |
| `gate.ingress.className` | The ingress class that serves `host`, when `gate.ingress.enabled`. |
| `vault.key` | The vault key: only the `AGE-SECRET-KEY-` line of `age-keygen`, one per install. |
| `crabbox.agent37Token` | The provider credential, read from `/var/lib/moor/config/agent37.key`. |
| `slack.appToken`, `slack.botToken` | The Slack app's tokens, when `slack.enabled`. Read [Slack](slack.md). |

Keep `vault.key`, the tokens, `crabbox.config` and `crabbox.sshKey` in a
values file you encrypt or hand to Helm from a secret store; the chart puts
them in one Secret, `<release>-secrets`.

`crabbox.config` is the crabbox config, as a map or as the file text. When
`crabbox.sshKey` is set, the chart writes it to
`/var/lib/moor/config/box-ssh-key` and points `ssh.key` at it. For agent37:

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
        owner: <owner>
        region: <region>
        keyFile: /var/lib/moor/config/agent37.key
        sshKey: /var/lib/moor/config/box-ssh-key
```

## Optional blocks

The chart's `values.yaml` documents each key. The blocks you are most likely
to turn on:

- `gate.ingress` exposes the gate's paths on `host`. `gate.login` adds browser
  sign-in at `/auth`; register `https://<host>/auth/callback` on the client
  first.
- `slack.enabled` runs the [Slack front](slack.md) beside the controller.
- `notify.enabled` runs the [webhook front](notify.md).
- `runner.enabled` runs the machine runner, so [machines](machines.md) can
  enroll through the gate.
- `controller.nodes` and `controller.defaultNode` declare shared
  [nodes](nodes.md).
- `egress.enabled` moves the shared egress Proxy out of the controller's pod.
  Read [The egress Proxy](#the-egress-proxy).

## The egress Proxy

Every Box's credential hosts go through one shared egress Proxy. By default the
controller runs it inside its own pod, so each upgrade cuts every Box's egress
until the new pod is up.

`egress.enabled` runs the Proxy as the Deployment `<release>-egress`. It has
two replicas, rolled one at a time, and each gives open tunnels
`egress.graceSecs` (300) to finish. Boxes dial it at `wss://<host>/egress` (`egress.url`
overrides it), which the gate ingress routes to the Service `<release>-egress`.
The Proxy reaches the controller's ledger over TLS at
`<release>-ledger.<namespace>.svc:7443`. A NetworkPolicy opens that port only
to the egress pods. The egress pods get the egress key and the CA, never the
database URL, the vault key or the provider keys.

It needs three more secrets, which the chart refuses to render without:

| Key | What it is |
| --- | --- |
| `egress.key` | The key Box tokens are signed with: `openssl rand -hex 32`. |
| `egress.ledgerTls.cert`, `egress.ledgerTls.key` | The certificate the ledger presents, issued for `<release>-ledger.<namespace>.svc`, and its key, in PEM. |
| `egress.ledgerTls.ca` | The PEM CA that signed it, which the egress pods trust. |

Make the CA and the certificate with openssl, for release `moor` in namespace
`moor`:

```sh
name=moor-ledger.moor.svc
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 3650 \
  -subj "/CN=moor ledger CA" -keyout ca.key -out ca.crt
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -subj "/CN=$name" -keyout ledger.key -out ledger.csr
openssl x509 -req -in ledger.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 3650 \
  -extfile <(printf 'subjectAltName=DNS:%s\nextendedKeyUsage=serverAuth\n' "$name") \
  -out ledger.crt
```

Put `ledger.crt`, `ledger.key` and `ca.crt` in the values. Keep `ca.key`
offline until the certificate needs renewing.

The machine runner's service (`machines.service`) is reached only from inside
the controller's pod. With `egress.enabled`, a Box's requests to it fail.

## Upgrades

The pod uses the `Recreate` strategy, so the old controller stops before the
new one starts. On stop the controller drains for up to
`controller.drainTimeoutSecs`, 60 seconds by default, and the gate drains for
the same time. The pod's grace period is the drain plus 30 seconds, so the
drain finishes before Kubernetes kills the pod. The new pod starts only after
the old one is gone, so a longer drain makes each upgrade's outage longer. Read
[Deploy the controller with systemd](deploy.md) for what a drain waits on.
