# Helm values

The chart is `oci://ghcr.io/inizio/charts/moor`. Its
[values.yaml](../deploy/helm/moor/values.yaml) carries a comment on every key.
This page groups the keys by what they run. Keys marked secret belong in an
encrypted values file. The chart puts them in the Secret `<release>-secrets`.

## Required

| Key | Meaning |
| --- | --- |
| `host` | The gate's public hostname. |
| `authgear.issuer`, `authgear.clientId` | The OIDC issuer and client. |
| `mcp.resourceUrl` | `https://HOST/mcp`, the API resource tokens are minted for. |
| `vault.key` | Secret. The `AGE-SECRET-KEY-` line of `age-keygen`. |

## Image and controller

| Key | Default | Meaning |
| --- | --- | --- |
| `image.repository`, `image.tag` | `ghcr.io/inizio/moor`, the chart's version | The release image. |
| `controller.nodes` | `{}` | Shared nodes, as `{name: {runtime, labels, max_boxes}}`. Empty runs Boxes only on paired machines. |
| `controller.defaultNode` | empty | `box.node`. Needed when `nodes` has more than one entry. |
| `controller.drivers` | `{}` | Per-provider settings, such as `external: {strip_env: [X]}`. |
| `controller.agentModel` | empty | The model a Box's agent starts with. |
| `controller.drainTimeoutSecs` | 30 | The controller's drain on stop. The pod's grace is this plus 30 s. |
| `controller.settleEverySecs` | 5 | Seconds between settle passes. |
| `controller.extraEnv` | `[]` | Extra environment, as `{name, value}` entries. |
| `crabbox.config` | none | Secret. The crabbox config, as a map or as text. |
| `crabbox.sshKey` | empty | Secret. The ssh key the provider account trusts, written to `/var/lib/moor/config/box-ssh-key`. |
| `crabbox.agent37Token` | empty | Secret. Written to `/var/lib/moor/config/agent37.key`. |
| `persistence.size`, `persistence.storageClassName` | `10Gi`, the default class | The `moor-state` volume. |

## Gate

| Key | Default | Meaning |
| --- | --- | --- |
| `gate.ingress.enabled`, `gate.ingress.className` | off | Expose the gate's paths on `host`. |
| `gate.login` | `false` | Browser sign-in at `/auth`. Needed by the account page, webhook setup, and machine pairing. |
| `gate.cookieKey` | made on first install | Seals sign-in cookies. Comma-separate keys to rotate. |
| `gate.jwksUrl` | empty | The issuer's keys, when discovery is unreachable. |
| `gate.separate` | `false` | Run the gate as `gate.replicas` pods of its own. Needs `egress.enabled`. |
| `gate.drainSecs` | 60 | With `separate`, how long a stopping gate lets requests finish. Keep it above 30. |
| `mcp.enabled` | `true` | Serve `/mcp`. |

## Fronts and the runner

| Key | Default | Meaning |
| --- | --- | --- |
| `slack.enabled` | `false` | Run the Slack front. Needs `slack.appToken` and `slack.botToken`, both secret. |
| `slack.separate` | `false` | Run Slack as `<release>-slack` on a volume of its own. Needs `egress.enabled`. |
| `notify.enabled` | `false` | Run the webhook front. |
| `runner.enabled` | `true` | Run the machine runner, so teammates can pair machines. |
| `runner.separate` | `false` | Run the runner as `runner.replicas` pods of its own. Needs `egress.enabled`. |
| `runner.agentsPort`, `runner.linksPort` | 7701, 7702 | Where agents link, and where the controller reaches a runner pod. |
| `runner.extraArgs` | `[]` | More `moor-machine-runner` flags, such as `--relay`. |
| `podDisruptionBudget.enabled` | `false` | One budget for the gate and runner pods. Needs RBAC on `policy/v1`. |

## Egress Proxy

| Key | Default | Meaning |
| --- | --- | --- |
| `egress.enabled` | `false` | Run the shared egress Proxy as the Deployment `<release>-egress`. Needs `gate.ingress.enabled` or `egress.url`. |
| `egress.replicas` | 2 | Replicas, rolled one at a time. |
| `egress.url` | `wss://HOST/egress` | Where Boxes dial the Proxy. Must be `wss://`. |
| `egress.graceSecs` | 300 | How long open tunnels get after SIGTERM. |
| `egress.ledgerPort` | 7443 | The controller's ledger port for the egress pods. |
| `egress.key` | made on first install | Secret. The key Box tokens are signed with. |
| `egress.ledgerTls.*` | made on first install | Secret. Your own ledger certificate, key, and CA. |

On first install the chart makes the Secret `<release>-egress`. It holds the
egress key, a ledger CA with its key, the ledger's certificate, and client
certificates for the egress, runner, and Slack pods. The certificates last ten
years. Each upgrade reads them back, and an uninstall keeps the Secret.

To bring your own, set `egress.key` and all of `egress.ledgerTls.cert`, `key`,
and `ca`. Then set either `caKey`, a PKCS#1 RSA key the chart signs the client
certificates with, or every client certificate and key. Any other mix fails
the render.

To rotate everything, delete the Secret, upgrade, then restart the pods:

```sh
kubectl -n NAMESPACE delete secret RELEASE-egress
helm upgrade ...
kubectl -n NAMESPACE rollout restart deployment/RELEASE deployment/RELEASE-egress
```

## Database and blobs

| Key | Default | Meaning |
| --- | --- | --- |
| `database.url` | empty | Secret. A Postgres URL. Empty keeps SQLite on the volume. |
| `database.readers` | 4 | Read connections the controller keeps open. |
| `blobs.s3.endpoint`, `blobs.s3.bucket` | none | An existing S3-compatible bucket for artifacts, Reports, and transcripts. |
| `blobs.s3.accessKey`, `blobs.s3.secretKey` | none | Secret. The bucket's keys. They need list, read, and write. |
| `blobs.copyFromVolume` | `false` | Copy blobs from the volume to the bucket once. |

With `database.url` and `blobs.s3` both set, the controller mounts no volume
and rolls with `maxSurge: 1`.

### Blobs

To move blobs from the volume to the bucket:

1. Set `blobs.s3` and `blobs.copyFromVolume: true`, and deploy. The Job
   `<release>-blob-copy-<hash>` starts once the old pod lets go of the volume.
2. Until the Job ends, older artifacts answer that the blob is missing.
3. Check `kubectl logs job/RELEASE-blob-copy-HASH`. The Job never overwrites a
   key, so a rerun copies only what is missing.
4. Drop `blobs.copyFromVolume` and deploy. Delete the old volume by hand once
   you no longer want it.
