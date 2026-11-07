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
two replicas, rolled one at a time, and spread across nodes where it can. A
rollout starts the next replica and waits for it to turn Ready before it stops
an old one. Each gives open tunnels
`egress.graceSecs` (300) to finish. Boxes dial it at `wss://<host>/egress`,
which the gate ingress routes to the Service `<release>-egress`. Without
`gate.ingress.enabled`, set `egress.url` to where Boxes reach that Service.

Every ten seconds each replica learns every awake Box, its secrets and its
machine from the ledger. While the controller is away, such as during its own
upgrade, any replica serves any open Box from those answers. A replica turns
Ready only after one whole pass finishes within eight seconds, so a rollout during a controller upgrade
waits and the old replicas keep serving.
The Proxy reaches the controller's ledger over mutual TLS at
`<release>-ledger.<namespace>.svc:7443`. The controller checks the Proxy's
client certificate against the ledger CA and drops any connection without one
in the handshake. The egress pods get the egress key, the CA and their client
certificate, never the database URL, the vault key or the provider keys.

The chart makes the egress secrets itself. On first install it creates the
Secret `<release>-egress` with these keys, the certificates valid for ten years:

| Key | What it is | Mounted in |
|---|---|---|
| `egress-key` | The key Box tokens are signed with | controller and egress pods, as `MOOR_EGRESS_KEY` |
| `ledger-ca.pem`, `ledger-ca.key` | The ledger CA and its private key | the CA: controller and egress pods. The key: nowhere |
| `ledger-tls.crt`, `ledger-tls.key` | The ledger's certificate for `<release>-ledger.<namespace>.svc` | controller |
| `egress-client.crt`, `egress-client.key` | The egress pods' client certificate, CN `moor egress` | egress pods |
| `runner-client.crt`, `runner-client.key` | A machine runner's client certificate, CN `moor runner` | nowhere yet |
| `machine-service-key` | The key the runner and the egress pods share | runner, egress pods |

Each upgrade reads them back, so they stay the same. The chart keeps the CA's
key so an upgrade can sign a certificate that is missing, from the same CA. An
uninstall leaves the Secret behind, since running Boxes hold tokens signed with
the key. A kept Secret whose CA has no key keeps its certificates as long as
all of them are there. If one is missing the render fails, since nothing can
sign it; set `egress.ledgerTls` again, or delete the Secret to start over.

To bring your own, set `egress.key` and all of `egress.ledgerTls.cert`,
`egress.ledgerTls.key` and `egress.ledgerTls.ca` (PEM). With them set either
`egress.ledgerTls.caKey`, from which the chart signs both client certificates,
or all of `egressClientCert`, `egressClientKey`, `runnerClientCert` and
`runnerClientKey`, signed by that CA as X.509 v3 certificates; the ledger refuses
a v1 certificate. Values you set win over the kept ones.
Any other mix fails the render. While the kept CA is the one you set, the
certificates the chart signed are read back, not signed again. `caKey` must be
a PKCS#1 RSA key, `-----BEGIN RSA PRIVATE KEY-----`. Convert another RSA key
with `openssl rsa -in ca.key -traditional -out ca-rsa.key`. An EC key does not
work.

To rotate the key, the CA and every certificate, delete the Secret and
upgrade, then restart the controller and the egress pods:

```sh
kubectl -n <namespace> delete secret <release>-egress
helm upgrade ...
kubectl -n <namespace> rollout restart deployment/<release> deployment/<release>-egress
```

Boxes' old tokens stop working. Each Box gets a new token the next time the
controller delivers its forwarder.

With `runner.enabled` too, the runner's machine service listens on the pod's
address, behind the Service `<release>-machine` on port 7700, and the egress
pods send Boxes' `machine.moor.internal` requests there. The service names the
calling Box from the `X-Moor-*` headers the Proxy sets, so it trusts them only
alongside `X-Moor-Service-Key`, a key the chart derives from `egress.key` and
gives the runner and the egress pods.

### What the chart renders, and what it leaves to you

With `egress.enabled` the chart adds the Secret, the Deployment and the
Service `<release>-egress`, and the Services `<release>-ledger` and, with
`runner.enabled`, `<release>-machine`. It renders no PodDisruptionBudget and no
NetworkPolicy, so it installs with namespaced RBAC on Secrets, Services,
ConfigMaps, Deployments and Ingresses alone. The rollout strategy above keeps a
replica serving through a deploy without either.

An operator who may manage `policy/v1` can keep a replica up through node
drains too:

```yaml
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: <release>-egress
spec:
  minAvailable: 1
  selector:
    matchLabels:
      app.kubernetes.io/name: moor-egress
      app.kubernetes.io/instance: <release>
```

With `gate.separate`, the same applies to `app.kubernetes.io/name: moor-gate`.

An operator who may manage `networking.k8s.io` NetworkPolicies, on a cluster
whose network plugin enforces them, can admit only the egress pods to the
ledger and machine ports. Keep the gate and agent ports open if you use them:

```yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: <release>-ledger
spec:
  podSelector:
    matchLabels:
      app.kubernetes.io/name: moor
      app.kubernetes.io/instance: <release>
  policyTypes: [Ingress]
  ingress:
    - from:
        - podSelector:
            matchLabels:
              app.kubernetes.io/name: moor-egress
              app.kubernetes.io/instance: <release>
      ports:
        - { port: ledger, protocol: TCP }
        - { port: machine, protocol: TCP }
    - ports:
        - { port: gate, protocol: TCP }
        - { port: agents, protocol: TCP }
```

The ledger refuses a client without the egress certificate either way. The
NetworkPolicy only narrows who can try.

## Runner Deployment

`runner.separate: true`, with `runner.enabled` and `egress.enabled`, moves the machine runner out of the
controller pod into `<release>-runner`, a Deployment of `runner.replicas` pods, two by default, that rolls
with `maxUnavailable: 0`. Each pod dials the controller's ledger with the machine service key, the ledger CA
and its own client certificate, `runner-client.crt` and `.key` from the Secret `<release>-egress`, and tells
the controller its own address, so a machine's link can sit on any replica. Its state is an `emptyDir` of Box
mirrors, so a new pod copies a Box's tree again on first use.

The Service `<release>-machine` then selects the runner pods, and the ingress sends `/machines/link` to
`<release>-runner-agents`. The controller dials a runner pod on `runner.linksPort`, 7702 by default, since
7701 is the agents port.

## Slack Deployment

`slack.separate: true`, with `slack.enabled` and `egress.enabled`, moves Slack out of the controller pod into
`<release>-slack`, one replica with `Recreate`, on a PVC of its own, `slack.persistence.size`, 1 GiB by default.
It holds `slack.db` and each thread's workdir, and it starts empty. Turning the flag on loses what the sidecar's
`slack.db` knew, which stays unread on the state volume:

- Existing threads lose their context. Their Boxes stay open on the controller, but those threads no longer
  follow them, so status, artifacts and questions stop appearing there.
- Questions a Box asked before the switch can no longer be answered from Slack.
- Each channel's repos are gone, so add them again with `/moor repo`.
- Workflows added before the switch keep running, but their runs are no longer reported in Slack.

The pod dials the controller's ledger with `slack-client.crt` and `.key` and the key `slack-front-key`, all from
the Secret `<release>-egress`. A `forward` container listens on loopback on `slack.proxyPort`, 4480 by default,
and carries each turn's traffic to the shared egress Proxy at the address Boxes dial. The pod holds no user's
secret.

## Controller without the volume

`blobs.s3` names a bucket for artifacts, Reports and transcripts:

```yaml
blobs:
  s3:
    endpoint: https://s3.example.com
    bucket: moor
    accessKey: ""   # secret
    secretKey: ""   # secret
```

The keys go into the Secret `<release>-secrets` as `blobs-access-key` and `blobs-secret-key`, and reach the
controller as `MOOR_BLOBS_ACCESS_KEY_ID` and `MOOR_BLOBS_SECRET_ACCESS_KEY`. The bucket must already exist.
Read [Blob storage](config.md#blob-storage) for what the controller does with it.

With `blobs.s3` and `database.url` both set, the controller pod mounts no volume. Its state directory is an
`emptyDir`, the chart no longer renders `moor-state`, and the pod rolls with `maxSurge: 1` and
`maxUnavailable: 1`. The new pod waits on the database lock and is not Ready until the old one exits, so
`maxUnavailable: 0` would never finish. Slack must then run with `slack.separate`, since the sidecar keeps
`slack.db` on the volume. The chart refuses to render otherwise.

A roll loses the rest of the state directory. The next pod remakes sockets, lock files, crabbox claims and
`pool/known_hosts`. Setup, clone and run logs of earlier pods are gone.

The old `moor-state` stays in the namespace, kept by its resource policy. Delete it by hand once its blobs are
in the bucket.

## Upgrades

The pod uses the `Recreate` strategy, so the old controller stops before the
new one starts. Without the volume, it rolls instead, as above. On stop the controller drains for up to
`controller.drainTimeoutSecs`, 30 seconds by default, and the gate drains for
the same time. The pod's grace period is the drain plus 30 seconds, so the
drain finishes before Kubernetes kills the pod. The new pod starts only after
the old one is gone, so a longer drain makes each upgrade's outage longer. Read
[Deploy the controller with systemd](deploy.md) for what a drain waits on.

`gate.separate: true`, with `egress.enabled`, moves the gate out of the
controller pod into `gate.replicas` pods, two by default, with no volume. They
dial the ledger listener with a generated certificate. A controller upgrade
leaves them up, and they hold `/mcp` and `/front` for up to 30 s while the
controller is away. Set `gate.drainSecs` above that hold. The chart keeps the
machine runner reachable through `<release>-agents` when it is still a sidecar.

`podDisruptionBudget.enabled: true` adds a PodDisruptionBudget with `maxUnavailable: 1` for the gate, and for the
runner pods when `runner.separate` is on and `runner.replicas` is above 1. A node drain then evicts one at a time,
so one replica always answers while the other reschedules. It is off by default, because applying a
`policy/v1` PodDisruptionBudget needs RBAC that some installs do not grant. It renders nothing for a gate or
runner that still sits in the controller pod.

With `runner.separate`, a controller upgrade leaves the runner pods up, and rolling the runner pods one at a
time relinks each machine within a redial of its agent.

With `slack.separate`, a controller upgrade costs Slack one redial. A Slack upgrade replaces its one pod: Socket
Mode reconnects in the new pod and Slack resends what the old pod did not acknowledge.
