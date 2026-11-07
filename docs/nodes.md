# Nodes

A node is a place that runs Boxes. Every start lands on one node, and the
scheduler picks it.

## Two kinds of node

A shared node is listed in the controller's user config, and every user may
use it:

```yaml
nodes:
  fast:
    runtime: cloudhypervisor
    labels: [big]
    max_boxes: 8
box:
  node: fast
```

`runtime` is the crabbox provider the node's Boxes run on, `labels` are what a
Box may require of the node, and `max_boxes` caps its live Boxes. `box.node`
names the default shared node; it is required when `nodes` has more than one
entry. Shared nodes are optional. Without one, Boxes start only on personal
nodes.

A personal node is a machine its owner enrolled with a runtime. Only its owner
can use it. The node is named for the machine and is online while the machine's
agent is linked:

```sh
moor-machine-agent enroll --server https://moor.example.com --name hv --runtime cloudhypervisor
moor-machine-agent run --runtime cloudhypervisor --max-boxes 4
```

## Memory

Workload memory is not predictable, so a Box's size only sizes its VM; it does
not decide whether the Box fits. A personal node's agent reports the host's
`MemAvailable`, `MemTotal` and memory pressure (PSI `some avg10`) when it links
and every 10 seconds. When its crabbox provider lists the host memory each lease's VM holds
(`hostMemoryMiB`, 0 once the VM is suspended to disk), the report carries it per Box and
`moor node ls` shows the sum as `Boxes hold X GiB`; with a provider that does not, it shows nothing.
Reports and replies on the link ignore fields the reader does not know, so a
new report field needs no version bump; requests stay strict. A node has room for one more Box when:

- its last report is under 60 seconds old,
- `MemAvailable` less the host reserve, less 2 GiB for each Box still setting up
  there, leaves at least 2 GiB, and
- PSI `some avg10` is under 10, or the host has no PSI.

A Box that does not fit is Pending with `waiting for capacity on NODE: X GiB
free, needs 2 GiB`, `memory pressure on NODE`, or `no recent memory report from
NODE`. A node whose agent never reports memory admits by Box count alone.

When a start does not fit, the node suspends its one longest-idle Box and the
start waits for the next report. A start also waits when Boxes already queue on
the node, however much is free, with the status `NODE is making room; N Boxes
ahead`. It never suspends a Box that is working, one
waiting on a question, or one still starting. With no idle Box, nothing is
suspended. A suspended Box's status says which Box it made room for. While a
Box waits for capacity on a node, each memory report there also suspends its
longest-idle Box, at most one per 30 seconds, by the same rules, with the status
`made room for BOX`. A wake that does not fit makes room the same way, leaves
its Box Pending with the reason, and a later report with room wakes it.

### Resuming

Every resume passes one gate, whether a report promotes the Box or a person or
agent asks for it. An asleep Box resumes only when:

- its last report is under 60 seconds old, PSI `some avg10` is under 10, and the
  disk is above its floor, as for a new Box,
- no Box has come up since the last report, which would not count it yet, and
- `MemAvailable` less the host reserve, less 2 GiB for each Box still setting up,
  leaves what the Box needs, or nothing runs on the node and `MemAvailable` is at
  least 1 GiB more than what it needs.

What a Box needs is `resumeMemoryMiB`, when its crabbox provider lists that for
the lease of a suspended VM. Without it, it is the Box's whole guest memory. A
guest's ceiling can be far above its real use: a Box with a 23 GiB ceiling that
really uses 6 GiB would otherwise stay asleep beside any other Box on a 30 GiB
host. A Box whose size moor never learned asks what a new Box does, reserve and
all.

A Box that resumed stays out of the suspend order for 30 seconds, about three
reports, so that what it faults back in shows first. Critical memory ignores
this, since running out is worse than a flap.

`--reserve-mib` sets the reserve kept back for the host. It defaults to 4096 MiB
plus 10% of the host's memory:

```sh
moor-machine-agent run --runtime cloudhypervisor --reserve-mib 8192
```

## Pressure relief

Boxes already running can still run a node short of memory. Each report also
checks for that, and suspends at most one Box per node, with at least 30
seconds between suspensions there. A Box that cannot be suspended, because its
lock is held or the provider fails, does not end the attempt: the next one in
order is tried on the same report. A provider failure is recorded on the Box as
a `stays_up` event, which says why it was being suspended; the same failure
repeats there at most every 10 minutes.

Pressure is sustained when PSI `some avg10` is 10 or more on three reports in a
row, or when `MemAvailable` falls below the host reserve. The node then
suspends its longest-idle Box, with the status `memory pressure on NODE`. It
never suspends a working Box for sustained PSI alone. The node list shows since
when pressure has lasted.

Memory is critical when PSI `full avg10` is 5 or more, or `MemAvailable` falls
below 1024 MiB. Critical memory, or `MemAvailable` below the reserve with at
least two Boxes running, suspends a working Box as a last resort once no idle
Box is left to try. The early trigger exists because a suspension takes time: at
1 GiB/s, a 23 GiB snapshot started with 7 GiB of headroom survives about 217
MiB/s of growth elsewhere, and one started with 1 GiB only 31 MiB/s. Among
working Boxes the most recently started goes first. A Box with a run in flight
goes only when no other is left, since the suspension drops the run's
connection. The only running Box is never suspended while it works. The status
is `last-resort suspend: memory critical on NODE` or `last-resort suspend:
memory below the host reserve on NODE`, ending `, with a run in flight` when
that applies. That Box waits Pending and resumes before any other Box waiting on the node, with
no one needing to act.

A Box whose host lists its machine as stopped while its agent was mid-turn is
treated the same way, with the status `its machine stopped, as when its host
restarts`. A quiet one just goes asleep and waits to be asked for.

A suspended Box keeps its session: moor cannot ask a machine that is not running
about its agent, so reading its status decides nothing. On a wake moor asks the
Box. A running agent gets a new pane and keeps its terminal and every process.
Only an agent the Box says is gone is started again, from its stored session. A
Box that does not answer keeps its session. Each status sweep reads it once,
within the usual probe deadline. Once a read shows the agent running, it is
told. Once the Box says the agent is gone, moor starts it again, once. That
start holds the sweep for at most about 70 seconds: up to 60 for the tunnel, then
the launch and its check. A failed start is recorded and not tried again.
Whichever op woke or restarted it, once it has answered, an agent that stopped
mid-turn is told once for how long, whether its processes are intact or lost, that network
connections open at that moment were dropped, and to carry on with its work. Its
wall clock jumps.

If a waiting Box's machine is gone altogether, it takes its turn for a new one,
whose disk is empty. Moor records a `disk_lost` event, starts the agent again, and
tells it what is gone. Anything not pushed is lost. `moor start` on a Box that lost
its machine does the same.

Every Box still finishes. Pressure only decides when it runs.

## Disk

A personal node's agent also reports the free and total space of the disk that
holds the Boxes' data. The node keeps the larger of 10% of that disk and 20 GiB
free. Below it, a start waits with `low disk on NODE: X GiB free, needs Y GiB`.
A node whose agent reports no disk figures admits without a disk check.

Boxes' disks grow as they fill, so a node can run low while Boxes run. When a
report puts free disk below the floor, the node is low. moor records one
`node_disk_low` event for each owner with a Box on the node, Ready or asleep,
because an asleep Box's disk still takes space. An awake Box carries the event
when the owner has one. The event has the node's name, `free_mib` and
`total_mib`.

The node is no longer low once free disk is back at the floor plus 5% of the
disk, and each owner then gets `node_disk_recovered`. Reports in between add
nothing, so a node near the floor does not repeat the alert. A report older
than 60 seconds, or one without disk figures, changes nothing.

`moor node ls` marks a low node with `(low)` after its free disk. With no Box on
the node, that mark is the only signal, because the events belong to Boxes. A
shared node has no agent report, so moor never judges its disk. moor suspends
and deletes nothing for a low disk. See [notify](notify.md) and
[listen](listen.md) for how an owner hears.

## Which node a start gets

Without `--node`, the scheduler tries your personal nodes first, in the order
you enrolled them, and then the default shared node. It tries only the first
one in that order. An offline node refuses the start and says how long it has
been offline. When a shared node exists, the message names it so you can pick
it with `--node`. A full node does not send the Box elsewhere.

## Pending

A start on a node that is full, by Box count or by memory, waits there. The Box is Pending, with the reason
in its status, and it starts when a Box on that node closes, the node's link
comes back, or a memory report shows room.

On a node that reports memory, one queue holds starts and resumes: Boxes
suspended as a last resort first, then other resumes, then new starts, each
oldest first. A memory report takes at most one Box from it, and the next report
counts that Box. The pass stops at the first Box that still needs room, so a
small Box does not pass a big one ahead of it, and a wake asked for now waits
behind what is ahead of it. `moor show` gives a waiting Box's place in line. A Box
at the head that fails for its own reason, such as a provider error, does not
hold the others back.

Nothing makes room for the head except what already frees memory: idle Boxes are
suspended for it and working ones finish. A head that could not fit even with every
Box on the node gone, going by what each lists as `hostMemoryMiB`, is beyond reach:
other work on the host leaves too little. No Box is suspended for it, and Boxes
behind it that fit may pass. As soon as stopping what runs would make room for the
head, passing stops, so it cannot starve. When a running Box does not list what it
holds, nothing passes. The head waits with its reason and place until the host has
room or someone closes it.

### Closing an asleep Box

Just before an idle Box is suspended, moor checks it for unpushed work and collects
its reports and artifacts, for about 10 seconds in all, and keeps the answer. If any of
that fails, no answer is kept and the suspend goes ahead. A suspended guest cannot change, so the answer
stands until the Box next runs: any wake or new machine clears it, and so does
finding a Box stopped that moor did not suspend. Critical memory skips the check.

A Box whose agent is working or that has a run in flight is not checked and keeps
no answer, since it could write between the check and the snapshot. An idle Box's
background jobs could still write in that moment, and that work is not counted.

A close never takes a place in the queue or suspends another Box:

| What is known | The Box fits now | It does not |
| --- | --- | --- |
| Clean | Wakes, so its teardown runs and its caches are saved | Deleted unwoken; its reports and artifacts are already saved. Caches are kept if it [saved them as it slept](providers.md#saving-at-a-pause), otherwise it records a `caches_not_saved` event |
| Holds unsaved work | Refused, naming the work, unwoken | Same |
| Nothing | Wakes and checks | Refused with why |

`--force` closes without any of this, and records `caches_not_saved` when the Box
is asleep and did not save its caches as it slept.


## Warm starts

A later Box on your node can start from a checkpoint a Box of yours saved there, at
any commit. Checkpoints are never shared across users or across nodes.

A Box at the checkpoint's own commit skips the setup lines the checkpoint holds. A Box
at another commit runs every line again on the refreshed tree. A line whose guard skips
its work can leave the old commit's output, so read
[what rerunning does not cover](config.md#setup-checkpoints).

`moor node ls` says `warm starts on`, or `warm starts off:` with the reason. The
agent offers them only when its provider has `workspace-checkpoint` and
`workspace-fork` and `crabbox doctor` passes. See
[the cloud-hypervisor provider](providers.md#use-the-cloud-hypervisor-provider)
for the setting, which the agent adds for you and an owner's `false` turns off.

A chain is the checkpoints of one user, repo, machine class and cache plan: the clone,
each setup line over 20 seconds, and the last line. Each checkpoint is a delta over the one
before. It holds the guest memory in use at the save and what changed on disk since,
so a chain costs about what setup wrote to disk, plus the memory in use at each save.
A Box at a new commit reuses the chain, and may save its disk over the layer it forked.
A just-booted 16 GiB VM on ext4 took 1.07 GiB for its first checkpoint and 0.60 GiB for
the next, with `v52.0-moor.1`. A real setup costs more, and that is not yet measured.

Checkpoints share the node's disk floor. A setup below the floor saves no
checkpoint and warns `checkpoint_skipped`. A node below the floor with a Box on it
deletes checkpoints no Box holds back until it is above the floor again,
superseded chains first, then the least recently used. A checkpoint a host holds with no record here stays until you remove it.

## Choosing a node

`moor node ls` lists the shared nodes and your own: name, scope, runtime, online
or offline since, used and capacity, free memory, PSI and the age of the last
memory report, and labels.

```sh
timeout 600 moor start fix-typo --node hv
```

`--node NAME` takes a shared node or one of your own. A name that is neither
refuses the start with `no node named NAME` and lists the nodes you can use. The
MCP and Slack `start_box` tools take the same optional `node`.

## Requiring labels

A repo can ask for labels in `.moor.yaml`:

```yaml
node:
  labels: [gpu]
```

The node must carry every label. If no node you can use has them, the start is
refused and the message lists each node's labels. A repo can ask for labels but
never name a node: `.moor.yaml` accepts only `node.labels`, and the node's name
comes from the start request. A label is 1-32 lowercase letters, digits or `-`,
starting with a letter or digit.
