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
entry. A config with no `nodes` has one shared node built from `box.provider`.

A personal node is a machine its owner enrolled with a runtime. Only its owner
can use it. The node is named for the machine and is online while the machine's
agent is linked:

```sh
moor-machine-agent enroll --server https://moor.example.com --name hv --runtime cloudhypervisor
moor-machine-agent run --runtime cloudhypervisor --max-boxes 4
```

## Which node a start gets

Without `--node`, the scheduler tries your personal nodes first, in the order
you enrolled them, and then the default shared node. It tries only the first
one in that order. An offline node refuses the start and says how long it has
been offline, and the message names the shared node you can pick with
`--node`. A full node does not send the Box elsewhere.

## Pending

A start on a node that is full waits there. The Box is Pending, with the reason
in its status, and it starts when a Box on that node closes or the node's link
comes back. Boxes on a node start in the order they asked.

## Choosing a node

`moor node ls` lists the shared nodes and your own: name, scope, runtime, online
or offline since, used and capacity, and labels.

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
