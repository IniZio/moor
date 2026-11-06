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
- Resource holds stay in the runner that holds the machine's link, since every
  call for that machine passes through it. They end with its link. While a
  runner drains, its counted calls finish there and the next runner counts
  anew, a bounded overlap.

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
- Service for `/machines/link` to the runner pods. The controller dials the pod
  address each link row names, so no headless Service.
- The runner rolls on every nightly, since it shares the image tag. That is
  fine once a roll is a one-second relink.

### Slack

Slack is one sidecar on the controller pod and keeps `slack.db` on the volume.
It moves to its own Deployment in the same way once its state is in the store,
which is its own slice: audit its four migrations, then a lease row so one
replica folds the Slack stream while another stands by.

## Data shape

```
machine_links  machine_id, runner_addr, since, heard_at, draining_since
```

The table holds many machines and many runners. A row is written by the
controller when a runner reports, so the runner needs no store client. The
runner names itself with `--advertise`. A row reads as linked, draining or
offline, and only a linked row heard within 60 s routes. A newer link held by
another runner wins over an older one. No generation column: a runner's own
count restarts with it and cannot order links across runners; `since` can.

## Decisions

| Decision | Reason |
| --- | --- |
| Any-replica runner with a link registry, not single-active plus standby | A WebSocket cannot be handed to a standby. A redial to any ready pod is the takeover, and a leader adds nothing. |
| Make before break on SIGTERM | Zero downtime for a machine means its new link is up before its old streams end. |
| No op replay | Exec is not idempotent. The controller already reports a cut link; a 30 s drain makes that rare. |
| Holds stay in the runner that holds the link | The registry sends every call for a machine to one runner, so its count is the only count. A store lease would make every heavy call wait on the controller. |
| Keep `runner.sock` for VM and host installs | One binary, one protocol; the socket is a transport choice behind the same wire. |
| Slack is its own slice | Its state is still per pod. |

## Runner over the network

In one pod, the runner and the controller share a volume: three Unix sockets
and two host paths. As separate pods, each becomes a TCP connection that opens
with the runner key (`MOOR_RUNNER_KEY`, derived in the chart as
`sha256("moor runner hello:" + egress.key)`, so a leak of the 7700 service key
cannot be replayed on the controller), and the runner's one durable file set
moves to the store. VM and host installs keep the sockets.

```
runner     ──tls <release>-ledger:7443, hello{runner, key}──▶ controller
             show, machine-hold, machine-keys, machine-links, machine-box, box-rsync
controller ──tcp runner_addr, hello{key, links | forwards | conn n}──▶ runner pod
```

### Runner to controller

The runner dials the ledger's TLS listener and opens with a runner hello. The
controller hands that stream to the loop that serves `runner.sock`, as
`Caller::Runner`, so the requests, their authz and the drain keep their code.
The ledger's own hello still serves store calls only.

### Mirror and put without a shared disk

```
runner:  rsync -a --delete -e 'moor-machine-runner box-rsh <box>' box:<workdir>/ mirrors/<box>/
box-rsh  ──tls 7443──▶ controller: box-rsync {box, argv}, then bytes frames both ways
controller: Box lock, wake, the Box's ssh words + argv ──▶ rsync --server in the Box
runner:  mirrors/<box>/ ──rsh over the agent link──▶ machine        (as today)
```

A put is the same call the other way: the runner rsyncs its staged file to
`box:<workdir>/<dest>` with `--chmod=F644`. The controller takes only an
`rsync --server` argv whose path lies beneath the Box's work tree. The bytes
frames are the ones the `attach` relay already sends (binary, 64 KiB each).

Deleted: `box-mirror` and `box-put` (the runner is their only sender), the
grant's host directory, and the PVC subPath mounts.

Limits: the mirror lives in the runner pod's `emptyDir`, so a Box's first sync
after a runner roll copies its whole tree; later syncs send only changes. A put
loses its 32 MiB cap and keeps the 10 minute deadline a mirror has.

### Box ports to the machine

```
keeper     ──tcp runner_addr, forwards{box, machine, box ports}──▶ runner ──agent──▶ machine listens
machine connects to port p ──▶ runner ──"conn n p" on that carrier──▶ keeper
keeper     ──tcp runner_addr, hello{conn n}──▶ runner pipes the agent stream
keeper     pipes it to its own tunnel to Box port p
```

Every connection runs from the controller pod to the runner, the direction the
key already guards, and no Box port opens on the pod network. The keeper stays
in the data path, so machine and dev-tunnel forwards still blip on a controller
deploy, which this spec accepts. The keeper's
owner must agree to three changes: a forward names Box ports only; the keeper
reads `conn` lines on its carrier and dials back once per connection; it finds
the runner by the link row, not by `runner_socket`. A carrier cut by a drain
already reopens on the next keeper tick, now against the new row.

Deleted: the runner's dial to the controller's loopback ports, and the
`(box_port, host_port)` pairs.

### Box records in the store

```
machine_boxes  box_id, machine_id, slot, reach, facts, prepared, suspended_at, evicted, updated_at
```

`reach` and `facts` are JSON text: the controller stores them and never reads
them. One runner request, `machine-box`, gets, puts and drops a row and lists a
machine's rows. Closing a Box drops its row. Any replica then wakes a Box that
another prepared. Deleted: `boxes/` in the runner's state directory.

### link-dial

`dial_command` becomes `moor link-dial --runner <addr> <machine> <slug>`, with
the address from the link row when the ssh words are built. The key comes from
the controller's environment, which ssh passes to its ProxyCommand. A relink to
another runner ends the old stream within the drain, and the next ssh call
reads the new row. `wire/links.rs` routes to the row's address, not the single
local runner. Deleted: `MOOR_STATE_DIR` in the command.

### Chart

- `<release>-runner` Deployment as in Chart above, two replicas by default.
  Ports: agents (`/machines/link`), 7700 (MCP), 7701 (controller dials).
- `--advertise=$(POD_IP):7701` from the downward API.
- The `<release>-machine` Service from #1138 selects the runner pods. Network
  policy: 7701 from the controller pod only, 7700 from the egress pods, 7443
  from the egress and runner pods.
- `MOOR_MACHINE_SERVICE_KEY` and `MOOR_RUNNER_KEY` from the release secret,
  one key per purpose; the ledger CA mounted as the egress pods mount it.
- `emptyDir` for `mirrors/` and `tmp/`. No PVC.

### Decisions

| Decision | Reason |
| --- | --- |
| Runner hello on the ledger's TLS listener, not a new listener | Port, cert, Service and policy exist; the hello hands off to the runner loop, so the ledger stays store-only. |
| `box-rsync` over bytes frames, mirror in the runner pod | rsync stays incremental and binary over framing `attach` already uses, with no shared disk. |
| Keeper dials back per connection | No Box port leaves loopback; the Box's ssh master stays with the driver. |
| Box records in the store, not a PVC per runner | A PVC binds a Box to one pod; any replica must read it. |
| Advertise the pod IP | The link row already names the pod. |

## PRs, in order

1. **Drain and relink, single pod.** Done on `feat/machine-link-registry`.
2. **Link registry in the store.** Done on the same branch.
3. **Runner dials the controller over TLS.** Runner hello on the ledger
   listener, runner `--controller tls://…` with CA and key. Check: a test
   runner over TLS gets `show` and `machine-keys`; a wrong key is refused.
   About 400 lines.
4. **Box records in the store.** `machine_boxes` migration (SQLite and
   Postgres), `machine-box`, `boxes/` deleted. Check: prepare a Box, restart
   the runner on an empty state directory, wake it; no second `prepare`.
   About 500 lines.
5. **box-rsync.** Mirror and put through `box-rsh`; `box-mirror`, `box-put`
   and the host directory grant deleted. Check: sync and pull tests with the
   runner's state directory unreadable by the controller. About 600 lines.
6. **Controller dials the runner by address.** Runner listener on 7701 with
   the key hello for links; `wire/links.rs` and link-dial route by the row.
   Check: two runners in a test, a machine linked to the second; lease, dial
   and ssh reach it. About 500 lines.
7. **Forwards dial back.** The keeper change above, reviewed by the keeper's
   owner. Check: a Box port reached from the machine through a runner in
   another network namespace than the keeper. About 400 lines.
8. **Runner Deployment.** The chart above. Check: the k3s roll-probe with
   `RUNNER_EXEC='deploy/moor-runner -c runner'`, three runner rolls and three
   controller rolls, ops every 0.5 s and an exec in flight: zero failed ops,
   the exec completes, and the agent shows `not linked` for at most one redial.
9. **Slack.** State to the store, lease, own Deployment.

## To measure in PR 1 and 8

- Seconds a machine shows `not linked` across a nightly deploy.
- Machine ops failed across a runner roll with one exec in flight.
- Box events recorded across the roll: the target is none of the five kinds
  listed above.

## Out of scope

- Controller replicas and stateless gate sign-in (phase 6).
- Replaying a cut exec.
- A separate image or tag for the runner.
