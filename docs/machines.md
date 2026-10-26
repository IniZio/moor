# Machines

A machine is one of a user's own computers that a Box can use, reached from the
host over ssh. The Box never holds the machine's key; the host does.

## Machine runner

`moor-machine-runner` is an MCP server on the host. An agent in a Box reaches it
at `http://machine.moor.internal/mcp`: the Box's Proxy routes that host to the
runner and sets three headers on every request, which the runner trusts to name
the caller.

| Header | Value |
| --- | --- |
| `X-Moor-Box` | the Box's numeric store id |
| `X-Moor-Machine` | the attached machine's name |
| `X-Moor-Machine-Ssh` | the machine's ssh target, such as `moor@minion` |

A request missing any of them gets an MCP error that names the missing header.

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
