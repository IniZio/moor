# Node resources

Boxes on a node share its memory, CPU and disk. This spec gives each resource
one rule set and one decider. A person can then keep a Box per project and
start short-lived Boxes on top, with no broken Box and no dead host.

It serves the charter in #1362. It replaces the resource parts of #1084.

## Why

Fourteen resource incidents were recorded between 2026-10-15 and 2026-11-10.
Ten came from arrangement. A resource was counted wrong, or several parts
handled it and none owned it. One came from real shortage.

## Model

```
node agent ──report every 10 s──▶ controller ──actions──▶ node agent
  memory, pressure                  one decider             start, wake
  cpu pressure                      per resource            suspend
  disk by class                                             delete
  the reserve it applied

node agent service          outside the ceiling, so it always reports
Boxes slice                kernel ceiling = host memory − reserve
  └─ one scope per Box VM
```

- **Controller decides.** Admission, suspension and disk reclaim are pure
  functions over the node report and the node's Boxes. Nothing else decides.
- **Node acts and holds the ceiling.** The agent runs what the controller
  sends. The kernel caps the Boxes slice, so the cap holds while the
  controller is away.
- **Node may refuse, never start.** Before a start or wake, the agent reads its
  own memory and disk. It refuses when they are under the lines. A report can
  be 10 seconds old, and this closes that gap. The Box then waits as usual.
- **Every Box is alike.** A Box kept for months and a Box for one task follow
  the same rules. A workflow's previous Box closes when its next run starts.
- **Count is not a resource.** No rule counts Boxes.

## What each state holds

| State | Memory | Disk |
| --- | --- | --- |
| Starting, working, idle | What the VM uses | Box disk |
| Asleep | None | Box disk and memory snapshot |
| Asleep, snapshot dropped | None | Box disk |
| Closed | None | None. Its caches stay as shared caches |

`moor node ls` and `list_nodes` show these totals for each node.

## Memory

One set of lines, all measured from the reserve.

```
MemAvailable
     │
     │   admit      headroom ≥ need + gap
     │   ─────────────────────────────────  gap, so an admitted Box does not
     │   hold       no new start or wake    trip relief at once
reserve ──────────────────────────────────
     │   relieve    suspend the longest-idle Box
critical ─────────────────────────────────
     │   last resort: suspend the newest working Box, never the only one
     ▼
```

- **Headroom** is `MemAvailable` less the reserve.
- **Need** of a start is the most host memory an earlier Box of the same repo
  held on this node. A repo with no history needs the boot floor.
- **Need** of a wake is what its VM held when it slept.
- **Pressure** counts the same as today. Sustained pressure relieves. Full
  pressure is critical.
- **In flight.** Headroom also subtracts the need of every start and wake the
  controller has sent and the report does not show yet.
- **First relief is free.** A guest returns its free pages to the host by
  itself. Suspension starts only when that is not enough.
- **Reserve** has one source. The agent applies it to its own ceiling and
  reports it. The controller reads that value.

No user sets a size. No Box is killed. A Box that does not fit waits and says
why.

## Disk

One floor, one account, one reclaim order.

- **Floor.** The node keeps free the larger of 10% and 20 GiB, plus the memory
  of every running Box. The second part means a suspend always has room.
- **Account.** The report gives bytes for Box disks, memory snapshots, saved
  setups, caches, and everything else on the filesystem.
- **Reclaim.** Below the floor, on every report, the controller frees disk in
  this order. It stops at the floor plus 5%, so reclaim does not restart on
  the next report.

```
1  saved setups a newer one replaced
2  saved setups no Box holds, least recently used first
3  caches no open Box has attached, least recently used first
4  memory snapshots of the Boxes asleep longest
```

- **Never reclaimed.** The disk of an open Box. A cache an open Box has
  attached. A memory file a running VM still reads. Anything under an hour old.
- **A dropped snapshot** leaves the Box asleep on its disk. A wake boots it and
  restores its session. Running processes are gone, and the status says so.
- **Still below the floor.** Starts wait with the largest holders named,
  including disk that is not moor's.

## CPU

The report carries CPU pressure. A new start waits while CPU pressure stays
high. Wakes and running Boxes are not held.

The threshold comes from a measured run. This rule is built only if that run
shows starts failing under CPU pressure.

## Host ceiling

`moor-machine-agent up` writes a user slice for Boxes beside the agent's
service. Each VM starts as its own scope in that slice. It needs no sudo.

- `MemoryHigh` is host memory less the reserve. Above it the kernel slows the
  Boxes and reclaims from them. The host and its other work stay responsive.
- `MemoryMax` sits above that as a backstop. It is reached only if suspension
  failed. The kernel then ends one VM. The agent sits outside the slice and
  is never the one ended.
- `CPUWeight` on the slice is below the default. A busy Box cannot starve the
  agent, so a loaded node still reports.

The ceiling protects the host from Boxes. It does not limit other work on the
host. [Add your machine](../add-machine.md) will say how to cap builds on a
machine that is also a node.

## Seen elsewhere

| Practice | Source | Use here |
| --- | --- | --- |
| Limit the workload group, keep the daemon outside it | Kubernetes allocatable and reserved groups | Boxes slice beside the agent |
| A slow-down limit under an outside monitor, a hard limit as last defence | Linux cgroup v2, systemd | `MemoryHigh` then `MemoryMax` |
| Node may refuse a create | E2B | Node refusal |
| Count placements in flight | E2B | In-flight need |
| Refuse a pause when disk cannot hold the memory | E2B | Suspend room in the floor |
| Evict to a mark above the trigger | Kubernetes image cleanup, 85% to 80% | Reclaim to floor plus 5% |
| Resume without a memory snapshot is a cold boot on the disk | Fly Sprites, agent-substrate | Dropped snapshot |
| Never evict a cache in use, keep a minimum age | agent-substrate image cache | Never reclaimed |

Most of these systems admit by declared size. moor keeps live memory, because
users cannot predict build memory. The learned need and the node refusal cover
the risk that choice carries.

## Today

| Resource | Deciders today | Result |
| --- | --- | --- |
| Disk | Controller floor evicts saved setups only. The provider's cache store has its own budget. The machine runner has its own floor. | Caches and Box disks pass the floor. A budget once evicted every cache on a node. |
| Memory and disk | Suspension writes a snapshot and never checks disk. | Relieving memory can fill the disk. |
| Memory lines | Admission and relief use separate constants. A start is charged 2 GiB whatever it will use. | Heavy Boxes are admitted together, then suspended. |
| Box count | Controller `max_boxes`, agent pool limit, and a `moor up` formula. An asleep Box holds a slot in each. | A sleeping Box blocks a start though it holds no memory. |
| CPU | Reported for display. | No rule. |
| Host | No limit on the agent's service. | A host ran out of memory twice on 2026-11-01. |

## Steps

Each step ends in a check on a real node.

| Step | Change | Check |
| --- | --- | --- |
| 0 | Record which cgroup VMs sit in today. Run busy Boxes until relief fires and read which line fired. | Recorded cgroup path and event |
| 1 | Boxes slice with its ceiling, one reserve, node refusal. | Boxes asked for more than the host has. Host memory stays above the reserve. No process outside the slice ends. The agent keeps reporting. Repeat with the controller disconnected and record whether any VM ends. |
| 2 | Disk account in the report. Floor includes suspend room. Reclaim runs on every report. | Fill the disk with saved setups. Free disk returns above the floor with no start refused. |
| 3 | Caches join the reclaim order. Delete the cache budget flag and the runner's own floor. | Fill the disk with caches. Oldest caches go, open Boxes keep theirs. |
| 4 | Drop snapshots of long-asleep Boxes. | A Box asleep with a dropped snapshot wakes with its session restored. |
| 5 | Delete Box count limits. | Twenty asleep Boxes on one node. A new start is admitted on memory alone. |
| 6 | One set of memory lines. Need learned per repo. | Three heavy Boxes start together. They run one after another. None is suspended while working. |
| 7 | CPU gate, if step 0 shows the need. | Starts wait under CPU pressure and begin when it clears. |
| 8 | The charter's proof. | One Box per project kept for a week with short-lived Boxes on top. All bars met. |

Steps 1 and 2 share no code. Step 3 needs the provider adapter to list and
delete caches with their size and last use. Step 4 needs new hands for a Box
from [Box hands](box-hands.md).

## Decisions

- **Controller is the only decider.** A second decider on the node would repeat
  the rules and drift from them.
- **Kernel ceiling, not a node-side policy.** It holds without the controller
  and needs no code path of its own. The node's refusal is a check, with no
  rules of its own to drift.
- **Only the controller suspends.** Choosing a Box to suspend needs to know which is idle, and only the controller knows. With the controller away, Boxes above the ceiling slow down. If step 1 shows a VM ending in that case, the controller will send its suspend order ahead and the node will use it while the controller is silent.
- **No disk I/O limit.** A user service cannot set one without root.
- **No Box classes.** Persistent and short-lived Boxes differ only in when a
  person closes them.
- **No count limits.** They made a sleeping Box block a start.
- **Need is learned, never declared.** Users cannot predict build memory.
- **Floor, not budget.** A budget evicted every cache while the disk had room.
- **Snapshots go before starts wait.** Losing running processes of a Box asleep
  for days costs less than refusing new work.
- **`MemoryMax` stays as a backstop.** Ending one VM is better than the host
  ending every VM, the agent and unrelated services.

## Not yet verified

- Which cgroup VMs sit in today, and that the provider can start each in a
  scope of the Boxes slice.
- A guest that is refused memory by the ceiling slows down and does not fault.
- Which memory files a restored VM still reads.
- The early relief line has fired on a real node.
- The CPU load that failed Box clones on 2026-11-06 came from Boxes.
