# Scheduling

This page explains where a new Box runs, when it waits, and why moor sometimes
puts a Box to sleep that nobody asked to sleep. The short answer is that moor
admits Boxes by free memory, not by size, and makes room by suspending idle
Boxes to disk.

## Which node a start gets

A start that names a node goes there. Otherwise moor tries your personal nodes
first, in the order you paired them, then the default shared node. It tries
only the first in that order. An offline node refuses the start and says since
when. A full node makes the Box wait. It never sends the Box elsewhere.

A repo's `node.labels` narrow the choice to nodes that carry every label. If
none of your nodes do, the start is refused, and the message lists each node's
labels.

## Why memory, not size

A Box's size sets its VM's memory ceiling, but what the agent actually uses is
not predictable. A Box with a 23 GiB ceiling may use 6 GiB. Counting ceilings
would leave most of a host idle, so moor counts what the host has free.

A personal node's agent reports the host's `MemAvailable`, `MemTotal`, and
memory pressure (PSI `some avg10`) every 10 seconds. A node has room for one
more Box when all of these hold:

- Its last report is under 60 seconds old.
- `MemAvailable`, less the host reserve and 2 GiB for each Box still setting
  up, leaves at least 2 GiB.
- PSI `some avg10` is under 10, or the host has no PSI.

The reserve is 4096 MiB plus a tenth of the host's memory, and
`--reserve-mib` on `moor-machine-agent up` changes it. The agent applies the
reserve to its own Boxes slice and reports it with every memory report. The
controller admits against the reported value. It uses the 4096 MiB plus a tenth
only for an agent that reports none. A node whose agent reports no memory
admits by Box count alone. A shared node always does.

## Waiting and making room

A Box that does not fit waits as Pending, with the reason in its status, such
as `waiting for capacity on NODE: X GiB free, needs 2 GiB`. One queue per node
holds starts and wakes. A Box suspended as a last resort comes first, then
other wakes, then new starts, each oldest first. A small Box never passes a big
one ahead of it, so the head cannot starve.

To make room, the node suspends its longest-idle Box, at most one every 30
seconds. A suspended Box keeps its whole state on disk and wakes later as if
nothing happened. A Box that is working, waiting on a question, or still
starting is never suspended to admit another.

A woken Box needs what its VM held when it slept, which the provider reports.
It passes the same checks as a new Box before it resumes.

## Pressure relief

Running Boxes can still exhaust a host. Each report checks for that:

- **Sustained pressure.** PSI `some avg10` of 10 or more on three reports in a
  row, or `MemAvailable` below the reserve, suspends the longest-idle Box.
- **Critical memory.** PSI `full avg10` of 5 or more, or `MemAvailable` below
  1 GiB, may suspend a working Box once no idle Box is left. The most recently
  started goes first. The only running Box is never suspended while it works.

A Box suspended at work says so in its status, waits first in line, and
resumes with no one needing to act. Its agent is told how long it was paused
and that open network connections dropped. Every Box still finishes. Pressure
only decides when it runs.

This is a trade we prefer to the alternative. Killing a Box loses work, and
letting the host swap stalls every Box on it. Suspending one to disk costs that
Box some minutes and nothing else.

## Disk

A personal node keeps the larger of 10% of its disk and 20 GiB free. Below
that floor, a start waits with `low disk on NODE`. Each owner with a Box on the
node gets one `node_disk_low` event. Each gets `node_disk_recovered` once the
disk is back above the floor plus 5%. To free disk, only saved setups that no
Box holds are deleted, superseded ones first.

## Warm starts

A later Box of yours on the same node can start from a saved setup of an
earlier one, at any commit. A saved setup is never shared across users or
nodes, because it holds what your setup lines and profile wrote. A Box at
another commit runs every setup line again on the updated tree. A line that
skips its work when its output exists can then keep stale files.
[Set up a repo](repo.md#install-dependencies) shows how to guard a line.

`moor node ls` and `list_nodes` show whether warm starts are on, or why not.
