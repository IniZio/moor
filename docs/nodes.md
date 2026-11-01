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
and every 10 seconds. A node has room for one more Box when:

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
suspended. A suspended Box's status says which Box it made room for. A wake that
does not fit makes room the same way, leaves its Box Pending with the reason,
and a later report with room wakes it.

`--reserve-mib` sets the reserve kept back for the host. It defaults to 4096 MiB
plus 10% of the host's memory:

```sh
moor-machine-agent run --runtime cloudhypervisor --reserve-mib 8192
```

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
they asked; a memory report starts at most one, and the next report counts it.

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
