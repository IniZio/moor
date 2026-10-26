# Machines

A machine is one of a user's own computers that a Box can use, reached from the
host over ssh. The Box never holds the machine's key; the host does.

## Machine runner

`moor-machine-runner` is an MCP server on the host. An agent in a Box reaches it
at `http://machine.moor.internal/mcp`: the Box's Proxy routes that host to the
runner and sets four headers on every request, which the runner trusts to name
the caller.

| Header | Value |
| --- | --- |
| `X-Moor-Box` | the Box's numeric store id |
| `X-Moor-Machine` | the attached machine's name |
| `X-Moor-Machine-Ssh` | the machine's ssh target, such as `moor@minion` |
| `X-Moor-Machine-Slot` | the Box's slot on the machine, 0 to 6553 |

A request missing any of them, or with a malformed one, gets an MCP error that
names the header.

A machine is shared by several Boxes, one per slot. Every port of slot k is the
repo's port plus 10·k, the same number on the machine and in the Box.

Start it next to the controller, as the same host user:

```sh
moor-machine-runner --socket /path/to/moor.sock --state-dir ~/.local/state/moor-machine-runner
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--socket` | `$MOOR_SOCKET` | the controller's operator socket |
| `--state-dir` | | where the runner keeps one host mirror per Box |
| `--listen` | `127.0.0.1:7700` | the address the Proxy routes to |
| `--service-host` | `machine.moor.internal` | the `Host` the Proxy passes through, accepted besides the loopback names |
| `--resource NAME=CAP` | none | a capped resource each machine has, such as `build=1`; repeat for more |

A resource caps heavy work on a machine, such as native builds, at CAP calls at
once. It is counted per machine name across all Boxes, and only the runner
process tracks holds: restarting it frees them all. With no `--resource` flags
nothing is held.

The runner uses the host's `ssh` and `rsync` with the host user's own key and
`known_hosts`, in batch mode. It stores and passes no secret.

## Tools

`sync` copies the Box's work tree to `~/moor/<box id>` on the machine, in two
hops. The runner first asks the controller for a [`box-mirror`](cli.md#box-mirror)
into `<state dir>/mirrors/<box id>` on the host, then runs
`rsync -a --delete --exclude=.git` over ssh from that mirror to the machine.
Both hops send only what changed since the last sync, and files gone from the
Box go from the machine. `.git` is left out; `node_modules` is kept, so native
builds on the machine find their packages.

`exec` takes `command` and `timeout_secs` (default 600, at most 3600). It runs
`cd ~/moor/<box id> && <command>` on the machine through the machine user's
login shell, so that user's `PATH` applies. It returns `exit_code`, `output`
(the last 64 KiB of combined stdout and stderr), `truncated` when earlier output
was cut, and `timed_out`. A command past its timeout has its ssh killed, comes
back with `timed_out: true` and no exit code.

`exec` also takes `hold`, a list of resource names, and `hold_wait_secs`
(default 900, at most 3600). Before the command runs, the call takes one unit of
each named resource on its machine, in name order so two calls never deadlock.
An unknown name is refused at once. While the call waits it sends MCP progress
notifications, such as `waiting for build on minion: held by Box 12`. If the
wait runs out, the call returns an error with that reason and runs nothing. A
hold ends when the call returns, times out or its client hangs up, so a call
dropped while it waits never takes the hold.

`machine_info` returns `box`, `machine`, `slot` and `port_offset` (10 times the
slot). An agent runs its dev servers on the machine on the repo's port plus
`port_offset`.
