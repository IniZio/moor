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

## Upgrades

The pod uses the `Recreate` strategy, so the old controller stops before the
new one starts. On stop the controller drains for up to
`controller.drainTimeoutSecs`, but Kubernetes kills the pod at the end of its
grace period, 30 seconds by default. Read
[Deploy the controller with systemd](deploy.md) for what a drain waits on.
