# Egress log, phase 1

Docker Boxes move to default-deny egress, enforced in the agent place's network
namespace, with the Proxy as the only way out. Before anything is blocked, phase 1
records every host a Box reaches, so the allow presets come from real runs. Nothing
is blocked in this phase.

## The record

One row per host, port and path, kept per Box.

| Field | Meaning |
| --- | --- |
| `host` | The name the client asked for. A direct row holds the IP address, because the netns sees only packets. |
| `port` | The destination port. |
| `path` | `proxied` (tinyproxy dialled it), `credential` (tinyproxy sent it to the Proxy) or `direct` (it skipped tinyproxy). |
| `first_seen` | When moor first saw it, in Unix milliseconds. |
| `count` | Requests through tinyproxy, or new connections for `direct`. |

The machine agent keeps the rows in `/run/moor/egress.json` in the Box. The Box's
keeper in the controller reads that file every 30 s. When a row is new, it writes an
`egress_seen` event that lists the new rows and every row whose count changed. With
no new row, a changed count is written at most every 10 minutes. `moor events --box
<name> --kind egress_seen` lists them.

## How traffic leaves a Docker Box today

Both places share one netns, and the agent place owns it. Every login in either place
gets `HTTP(S)_PROXY`, which points at tinyproxy in the agent place. For each credential
host, tinyproxy goes through `moor-egress-forward`, which carries it to the Proxy. The
credential hosts are GitHub, Anthropic and the `proxy.rules` hosts. Every other host,
tinyproxy dials itself. A tool that ignores the proxy variables dials out directly. So do the
inner dockerd's pulls and the containers it runs, whose bridge lives in the same netns.

## Where each path is seen

- **proxied and credential.** On the machine path, tinyproxy logs at `LogLevel Connect`
  to `~/.moor/egress-proxy.log`. Its `Request` lines carry the host and port. moor reads
  only those two fields and never the URL path. A host that matches an `upstream` line of
  tinyproxy's config is `credential`. The machine agent reads the log from an offset and
  empties it past 1 MiB.
- **direct.** An nftables table, `inet moor_egress`, sits in the Box's netns. Its output
  hook skips loopback and any socket in the agent place's cgroup (tinyproxy,
  `moor-egress-forward`, Claude Code). Its forward hook catches the inner containers.
  Each new connection lands in a dynamic set keyed by address, protocol and port, with a
  counter per element. So the kernel dedupes and caps the set at 4096 per family.

## Who installs the rules

The agent place is not privileged and runs as uid 1000. The machine agent runs as a
user in the `docker` group, so `nsenter` (host root) is out. The machine agent instead
runs a short-lived helper from the agent image, with `--network container:<agent>
--cgroupns host --cap-drop ALL --cap-add NET_ADMIN`. `NET_ADMIN` reaches only that
netns. The host cgroup namespace lets the rule name the agent place's cgroup, read
from `/proc/<pid>/cgroup` on the host. The helper creates the table when it is missing,
then lists the sets. It runs at each read, so an agent place restart, which brings a new
netns, gets its rules back within 30 s. The counters then restart from zero, and moor
adds the new counts to what it kept.

The privileged work place shares the netns, so its root can flush the table or spoof
the log. That is acceptable while nothing is blocked. Phase 2 needs an enforcement
point the work place cannot reach.

## Gaps

- A direct row names an IP address, not the host.
- UDP and TCP to the same address and port share one row.
- A controller restart writes the rows again in its first event.
