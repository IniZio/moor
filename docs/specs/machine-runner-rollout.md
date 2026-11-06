# Machine runner rollout

Phase 8 of zero-downtime deploys (#768): a deploy or a node drain leaves every
machine linked, and no machine op fails for longer than one redial.

## What a deploy does to machines today

On 2026-11-06 the nightly deployed ten times between 02:19 and 06:50 UTC. Each
deploy replaced the one pod that holds the controller, gate, runner, Slack and
notify. engine-03's agent logged `machine 1 is not linked` within a minute of
every deploy, and its Boxes recorded, each time:

- `agent_unreachable` after 4 tries, then `agent_link_lost` and
  `agent_link_restored` on a new pane,
- `tunnel_restarted: tunnel exited`,
- `busy` and `idled` flips on a Box whose agent never stopped,
- once, a working Box suspended under `made room for` because its false idle
  made it look safe to suspend.

The runner exits 0 on SIGTERM with no drain, so every stream in flight on the
link (exec, hook, relay) is cut. The agent redials with 1, 2, 4 ... 60 s waits,
and the new runner refuses the hello while the controller socket is not up yet
(`server cannot check keys now`), so a 7 s pod gap becomes a 7 to 15 s unlink.

## Inventory: what the runner holds in memory

| Held | Lost on restart means |
| --- | --- |
| agent links (WebSocket plus yamux, one stream per op) | every stream cut |
| link generations and memory reports | admission waits for the next report |
| resource holds | "a runner restart frees them all" (docs/machines.md) |
| forward routes and relay sessions | machine tools drop from Box agents |
| `machine-links` push to the controller | controller sees every machine offline |

The runner reaches the controller over `runner.sock` on the pod's volume, and
the controller reaches the runner the same way. Nothing in the store says which
process holds a machine.

## Design

```
agent ──ws──▶ Service /machines/link ──▶ runner pod A   (link row: machine, pod A, gen)
                                        runner pod B
controller op for machine M ──▶ store: link row ──▶ dial pod A ──▶ stream
```

- The runner is its own Deployment. Any replica count, no leader, no standby.
  Each link lives in one pod, and the store says which.
- The runner keeps its wire protocol and keys. The controller dials the runner
  over TCP with the runner key, by the address in the link row, instead of
  `runner.sock`.
- Holds move to the store, keyed by machine and owner, with the holding pod and
  a lease time. A pod that is gone past its lease holds nothing.

### Handover on SIGTERM, make before break

```
SIGTERM ──▶ runner A: mark its link rows draining, stop admitting
        ──▶ tell each agent `relink`
agent   ──▶ dial Service again (new pod B is Ready: maxSurge 1, maxUnavailable 0)
        ──▶ link to B admitted, row flips to B, new ops route to B
        ──▶ old streams on A finish, agent closes A
runner A ──▶ exits when every link is gone or the budget (30 s) ends
```

A stream still running at the budget's end is cut and reported as today. No op
is replayed: an exec is not idempotent, and the controller already classifies a
cut link as a lost connection.

### Admission without the controller

The runner checks a machine's keys through the controller. During a controller
roll that call fails. The runner retries it for up to 10 s before refusing the
hello, so an agent that dials during a roll links as soon as the controller
answers, instead of backing off.

### Chart

- Runner Deployment: `RollingUpdate`, `maxSurge: 1`, `maxUnavailable: 0`,
  `terminationGracePeriodSeconds: 40`, a PodDisruptionBudget with
  `minAvailable: 1`, readiness when the agents port accepts and the controller
  answers.
- Service for `/machines/link` to the runner pods. A headless Service gives the
  controller each pod's address.
- The runner rolls on every nightly, since it shares the image tag. That is
  fine once a roll is a one-second relink.

### Slack

Slack is one sidecar on the controller pod and keeps `slack.db` on the volume.
It moves to its own Deployment in the same way once its state is in the store,
which is its own slice: audit its four migrations, then a lease row so one
replica folds the Slack stream while another stands by.

## Data shape

```
machine_links  machine_id, runner_addr, generation, linked_at, draining_since
machine_holds  machine_id, owner, kind, runner_addr, leased_until
```

Both tables hold many machines, many runners and many owners. A link row is
written by the controller when a runner reports, so the runner needs no store
client.

## Decisions

| Decision | Reason |
| --- | --- |
| Any-replica runner with a link registry, not single-active plus standby | A WebSocket cannot be handed to a standby. A redial to any ready pod is the takeover, and a leader adds nothing. |
| Make before break on SIGTERM | Zero downtime for a machine means its new link is up before its old streams end. |
| No op replay | Exec is not idempotent. The controller already reports a cut link; a 30 s drain makes that rare. |
| Holds in the store with a lease | A runner restart must not free every hold; a dead pod must not keep them. |
| Keep `runner.sock` for VM and host installs | One binary, one protocol; the socket is a transport choice behind the same wire. |
| Slack is its own slice | Its state is still per pod. |

## PRs, in order

1. **Drain and relink, single pod.** Runner drains on SIGTERM (stop admitting,
   `relink` to agents, streams finish within 30 s). Agent relinks at once on
   `relink` and resets its backoff. Key check retries for 10 s. Measured on a
   nightly: unlink per machine from about 10 s to about one redial.
2. **Link registry and holds in the store.** Runner reports carry its address;
   the controller writes `machine_links` and `machine_holds`; machine ops route
   by the row. Transport stays `runner.sock` in the pod.
3. **Runner over TCP.** The controller dials the runner by address with the
   runner key; the runner Deployment, Service and PDB in the chart; the k3s
   rollout probe from #769 extended with a linked fake agent and an exec in
   flight through the roll.
4. **Slack.** State to the store, lease, own Deployment.

## To measure in PR 1 and 3

- Seconds a machine shows `not linked` across a nightly deploy.
- Machine ops failed across a runner roll with one exec in flight.
- Box events recorded across the roll: the target is none of the five kinds
  listed above.

## Out of scope

- Controller replicas and stateless gate sign-in (phase 6).
- Replaying a cut exec.
- A separate image or tag for the runner.
