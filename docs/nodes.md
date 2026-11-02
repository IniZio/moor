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
and every 10 seconds. Reports and replies on the link ignore fields the reader does not know, so a
new report field needs no version bump; requests stay strict. A node has room for one more Box when:

- its last report is under 60 seconds old,
- `MemAvailable` less the host reserve, less 2 GiB for each Box still setting up
  there, leaves at least 2 GiB, and
- PSI `some avg10` is under 10, or the host has no PSI.

A Box that does not fit is Pending with `waiting for capacity on NODE: X GiB
free, needs 2 GiB`, `memory pressure on NODE`, or `no recent memory report from
NODE`. A node whose agent never reports memory admits by Box count alone.

When a start does not fit, the node suspends its one longest-idle Box and the
start waits for the next report. It never suspends a Box that is working, one
waiting on a question, or one still starting. With no idle Box, nothing is
suspended. A suspended Box's status says which Box it made room for. While a
Box waits for capacity on a node, each memory report there also suspends its
longest-idle Box, at most one per 30 seconds, by the same rules, with the status
`made room for BOX`. A Box still working holds the memory it has touched, since
a guest's size is fixed while it runs. A wake that does not fit makes room the
same way, leaves its Box Pending with the reason, and a later report with room
wakes it.

`--reserve-mib` sets the reserve kept back for the host. It defaults to 4096 MiB
plus 10% of the host's memory:

```sh
moor-machine-agent run --runtime cloudhypervisor --reserve-mib 8192
```

## Pressure relief

Boxes already running can still run a node short of memory. Each report also
checks for that, and suspends at most one Box per node, with at least 30
seconds between suspensions there.

Pressure is sustained when PSI `some avg10` is 10 or more on three reports in a
row, or when `MemAvailable` falls below the host reserve. The node then
suspends its longest-idle Box, with the status `memory pressure on NODE`. It
never suspends a working Box for sustained pressure. The node list shows since
when pressure has lasted.

Memory is critical when PSI `full avg10` is 5 or more, or `MemAvailable` falls
below 1024 MiB. An idle Box goes first then too. When none is left, the node
suspends the Box that started most recently among the working ones, as a last
resort, with the status `last-resort suspend: memory critical on NODE`. That
Box waits Pending and resumes before any other Box waiting on the node. When it
wakes, its agent is told once that it was suspended, for how long, and that
network connections open at that moment were dropped. Its processes and files
are intact, though its wall clock jumps.

Every Box still finishes. Pressure only decides when it runs.

## Which node a start gets

Without `--node`, the scheduler tries your personal nodes first, in the order
you enrolled them, and then the default shared node. It tries only the first
one in that order. An offline node refuses the start and says how long it has
been offline. When a shared node exists, the message names it so you can pick
it with `--node`. A full node does not send the Box elsewhere.

## Pending

A start on a node that is full, by Box count or by memory, waits there. The Box is Pending, with the reason
in its status, and it starts when a Box on that node closes, the node's link
comes back, or a memory report shows room. Boxes on a node start in the order
they asked, after any Box suspended as a last resort; a memory report starts at
most one, and the next report counts it.

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
