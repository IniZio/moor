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

A Box that sleeps frees its slot for other Boxes but keeps its claim on the same
machine, and its files there stay (`machine_detached` with `asleep: true`).
When it wakes it takes the lowest free slot on that machine, which may be a
different slot and so different ports (`machine_attached` names them). If every
slot is taken, the Box runs without the machine, `machine_waiting` says why
once, and the Proxy tries again on each tick until a slot frees or the Box
sleeps or closes. `moor show` names the machine as asleep or waiting meanwhile.
Closing the Box ends its claim.

Start it next to the controller, as the same host user:

```sh
moor-machine-runner --socket /path/to/moor.sock --state-dir ~/.local/state/moor-machine-runner
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--socket` | `$MOOR_SOCKET` | the controller's operator socket |
| `--state-dir` | | where the runner keeps one host mirror and one record per Box |
| `--listen` | `127.0.0.1:7700` | the address the Proxy routes to |
| `--service-host` | `machine.moor.internal` | the `Host` the Proxy passes through, accepted besides the loopback names |
| `--resource NAME=CAP` | none | a capped resource each machine has, such as `build=1`; repeat for more |
| `--prepare CMD` | none | run on the machine before a Box's first `exec` or `machine_info`; see [Per-Box setup](#per-box-setup) |
| `--release CMD` | none | run on the machine once a prepared Box is no longer open, before its copy there is removed |
| `--suspend CMD` | none | run on the machine when a prepared Box falls asleep; see [Sleeping Boxes](#sleeping-boxes) |
| `--min-free-gb` | `20` | free GB a machine needs before a Box is prepared or synced there; below it sleeping Boxes' files are removed, oldest first; 0 turns it off; see [Disk guard](#disk-guard) |
| `--hook-timeout-secs` | `600` | how long `prepare`, `suspend` or `release` may run |
| `--reconcile-secs` | `60` | how often the runner lists each machine's `~/moor` and `~/moor-state` for Boxes it has no record of |
| `--sweep-secs` | `60` | how often the runner looks for prepared Boxes that are no longer open and for idle relay sessions |
| `--relay NAME=CMD` | none | a stdio MCP server on the machine whose tools the runner lists as `NAME_<tool>`; repeat for more; see [Relays](#relays) |
| `--force-arg NAME.ARG=FACT` | none | set argument ARG of relay NAME's tools to the Box's fact FACT on every call, hidden from the Box; repeat for more |
| `--hide-tool NAME.TOOL` | none | leave relay NAME's tool TOOL out of the listing and refuse calls to it; repeat for more |
| `--relay-idle-secs` | `1800` | how long a relay session may go without a call before it ends |
| `--keep-alive-secs` | `15` | how often an open call's response stream gets a ping, and a running `sync` or `exec` sends a progress notification (`sync: running for N s`) when the call asked for progress, so neither the agent's MCP client nor a proxy on the way sees a long call as idle |

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
Box go from the machine. `.git` is left out, and so is every pattern in the
Box's [sync excludes](config.md#sync-excludes), which the Proxy sends in
`X-Moor-Machine-Exclude`. By default that leaves out `node_modules`, `Pods`,
`.gradle`, `DerivedData`, `.dart_tool` and `target`, so install packages on the
machine itself. `machine_info` returns the list as `sync_exclude`.

`sync` takes an optional `exclude`: up to 64 more rsync patterns relative to the
work tree root, such as `["app/android/app/build"]`. A call adds to the Box's
sync excludes and cannot remove one. Each becomes one
`--exclude=PATTERN` argument of both hops, so the Box never sends those paths
to the host mirror and the machine neither receives them nor loses them to
`--delete`. Use it for what the machine builds for
itself, such as `node_modules` installed there, which a Box on another platform
would otherwise overwrite. A pattern that is empty, starts with `-` or holds a
newline or NUL is refused by name. The first hop is unchanged.

`pull` copies one file from the machine into the Box's work tree, for something the
machine made, such as a screenshot a relayed tool saved or a build log. `path` is
absolute on the machine or relative to `~/moor/<box id>`; `dest` is relative to the
work tree and may not be empty, absolute or contain `..`. The runner brings the file
to a staging file on the host with rsync over ssh, then asks the controller to
write it with [`box-put`](cli.md#box-put). The file may be at most 32 MiB, and an
existing file at `dest` is replaced. It returns `dest` and `bytes`.

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

`machine_info` returns `box`, `machine`, `slot`, `port_offset` (10 times the
slot) and `facts`. An agent runs its dev servers on the machine on the repo's
port plus `port_offset`.

## Per-Box setup

Boxes that share a machine may each need their own things on it, such as their
own simulator device or build folder, gone once the Box closes. The operator
supplies two commands; the runner knows nothing of what they make.

`--prepare CMD` runs before a Box's first `exec` or `machine_info`. The runner
runs it on the machine over ssh, in `~/moor/<box id>` (made if missing), through
the machine user's login shell, with these variables set:

| Variable | Value |
| --- | --- |
| `MOOR_BOX` | the Box's id |
| `MOOR_SLOT` | the Box's slot |
| `MOOR_PORT_OFFSET` | 10 times the slot |
| `MOOR_STATE_DIR` | the Box's state dir, `~/moor-state/<box id>`, as an absolute path |
| `TMPDIR` | `$MOOR_STATE_DIR/tmp` |

The runner makes the state dir and its `tmp` before running anything for the
Box, and gives the same two variables to `exec` commands and to the processes
the relays start. Whatever a tool writes to its temp dir therefore lands under
the Box's state dir and goes when the Box does.

The last line CMD prints on stdout must be a JSON object of string values: the
Box's facts, such as `{"device": "<udid>"}`. Its stderr may say anything. If CMD
exits non-zero, runs past `--hook-timeout-secs` or ends on another line, the
call that needed it fails with the tail of its output, nothing is kept, and the
Box's next call runs it again. Two calls of one Box never run it at once.

The runner keeps the facts with the Box's ssh target and slot in
`<state dir>/boxes/<box id>.json`, so a restarted runner does not prepare the Box
again. `machine_info` returns them as `facts`, and every `exec` command sees
each as `MOOR_FACT_<KEY>`: the key uppercased, every character but letters and
digits as `_`, so `device` is `MOOR_FACT_DEVICE`. With no `--prepare` the facts
are empty and the record is still kept.

`--release CMD` runs, in the same place, with the same variables and the
`MOOR_FACT_*` ones, once a prepared Box is no longer open. A `sync` also leaves a
record, so a Box that only synced is cleaned up too. Every `--sweep-secs` the
runner asks the controller for the open Boxes (a `show` with no Box, over the
same socket). For each recorded Box the controller does not list, it runs
release if the Box was prepared, removes `~/moor/<box id>` and
`~/moor-state/<box id>` on the machine, then deletes the Box's record and its
host mirror `<state dir>/mirrors/<box id>`. If
a step fails, it keeps the rest, prints the tail of the output to stderr, and
tries again on the next sweep. If the controller cannot be asked, nothing is
released.

The sweep also cleans up Boxes the runner has no record of, such as after the
state dir was lost, a crash between a copy and its record, or a record removed
by hand. At start and then every `--reconcile-secs` (default 60), for each
machine the runner has served or holds a record for (it keeps their ssh targets
in `<state dir>/machines`; `machine-list` names one user's machines, so it
cannot list them all), one ssh call lists the entries under `~/moor` and
`~/moor-state`. Each entry named by a plain decimal Box id that is not open and
has no record gets `release` run with `MOOR_BOX`, `MOOR_STATE_DIR` and `TMPDIR`
set and no `MOOR_SLOT`, `MOOR_PORT_OFFSET` or `MOOR_FACT_*`, then both
directories removed. Mirrors under `<state dir>/mirrors` of closed Boxes with no
record go on every sweep. Entries with any other name are never touched, nor
are the files of an open Box. A failing step is logged and retried on the next
pass. So `release` must tolerate a run with no facts, and one for a Box whose
`prepare` never ran.

`sync` runs `rsync --delete` into `~/moor/<box id>`, so anything prepare puts
there is removed by the next sync. Put per-Box folders, such as a build's
derived data, under `$MOOR_STATE_DIR` instead; the runner removes it with the Box,
and release only has to undo what lives elsewhere.

An example for iOS work, where each Box gets its own simulator:

```sh
moor-machine-runner --socket /path/to/moor.sock \
  --state-dir ~/.local/state/moor-machine-runner \
  --prepare '~/bin/moor-prepare' --release '~/bin/moor-release'
```

`~/bin/moor-prepare` on the machine:

```sh
#!/bin/sh
set -e
mkdir -p "$MOOR_STATE_DIR"/DerivedData
udid=$(xcrun simctl list devices | sed -n "s/.*moor-$MOOR_BOX (\([0-9A-F-]*\)).*/\1/p" | head -1)
[ -n "$udid" ] || udid=$(xcrun simctl create "moor-$MOOR_BOX" "iPhone 16" "iOS18.5")
printf '{"device": "%s"}\n' "$udid"
```

`~/bin/moor-suspend` on the machine:

```sh
#!/bin/sh
xcrun simctl shutdown "$MOOR_FACT_DEVICE" 2>/dev/null
exit 0
```

`~/bin/moor-release` on the machine:

```sh
#!/bin/sh
[ -n "$MOOR_FACT_DEVICE" ] && xcrun simctl delete "$MOOR_FACT_DEVICE" 2>/dev/null
exit 0
```

An agent in the Box then builds with
`xcodebuild -destination "id=$MOOR_FACT_DEVICE" -derivedDataPath $MOOR_STATE_DIR/DerivedData`.

### Sleeping Boxes

A Box the controller lists as asleep (`lifecycle` `asleep` in the `show` reply)
frees its slot, and its processes on the machine should stop, but its files stay.
On each sweep, for every recorded, prepared Box that is asleep and not yet
suspended, the runner ends the Box's relay sessions, frees the capped resources
its calls hold (even a call still running), then runs `--suspend CMD`. CMD runs
where `release` runs, with the same variables, `MOOR_FACT_*` included, and the
slot the Box had; use it to stop what `prepare` started, such as a simulator. Only
after it succeeds does the runner note the time in the Box's record as
`suspended_at`; a failing suspend is logged and run again on the next sweep. With
no `--suspend` the Box is still marked, so the steps below apply.

When the Box wakes it may get a different slot, since the controller gives it the
lowest free slot on the same machine. Its next `exec`, `machine_info`, relay
call or `sync` runs `prepare` again, with the slot from the request, if the Box
was suspended or the request's slot differs from the record's. The new facts and
slot replace the old ones, `suspended_at` is cleared, and `machine_info` reports
the new slot, `port_offset` and facts. So `prepare` must be idempotent: running it
again for a Box that already has its device and folders must reuse them and print
the facts for the current slot, such as a Metro port that depends on
`MOOR_PORT_OFFSET`.

### Disk guard

`--min-free-gb N` (default 20; 0 turns it off) keeps the machine from filling up.
Before it runs `prepare` for a Box and before every `sync`, the runner reads the
free space in the machine user's home with one remote call, `df -Pk ~`. Below the
floor, it takes the machine's suspended Boxes that the controller still lists as
asleep, oldest `suspended_at` first, and for each runs `release`, removes
`~/moor/<box id>` and `~/moor-state/<box id>`, and re-reads the free space,
stopping once it is above the floor. It keeps the Box's record, marked `evicted`
with its facts cleared and `prepared` false, and its host mirror. It never
touches a Box that is awake, a Box in the middle of a call, nor anything outside
the Boxes' own directories, such as shared caches. If the space is still below the
floor with no sleeping Box left, the call fails with the free GB, the floor and
that no sleeping Box is left to evict.

An evicted Box's next call prepares it again. Until it syncs, `machine_info`
carries a `notice` saying its files on the machine were removed while it slept,
so it must `sync` again.

## Relays

A machine may run MCP servers of its own over stdio, such as Argent
(`argent mcp`) for driving simulators. `--relay NAME=CMD` lists such a server's
tools to every Box beside `sync`, `exec` and `machine_info`. NAME is
`[a-z][a-z0-9-]*`.

Each tool is listed as `NAME_<tool>`, with every character but letters, digits
and `_` turned into `_`: relay `argent`'s `list-devices` is
`argent_list_devices`. If two tools would get the same name, or one would take
a name the runner already lists, that relay's session refuses to start and its
tools are left out, with the reason on stderr and in the error of a call to one
of them. Results, errors and progress come back from the server unchanged.

`--force-arg NAME.ARG=FACT` keeps a Box on what was set up for it. For each of
relay NAME's tools whose input schema has ARG, the runner removes ARG from the
schema's `properties` and `required`, so the agent never sees it, and on every
call sets ARG to the Box's fact FACT, replacing anything the agent sent. A Box
without FACT gets an error naming it, and the call is not relayed.

Facts are strings, so the value sent follows the type the tool's schema gives
ARG:

| Schema type | Sent |
| --- | --- |
| `string`, or none | the fact as is |
| `integer`, `number` | the fact parsed as a JSON number; a fact that does not parse gets an error naming the fact and the tool, and the call is not relayed |
| `array` | a one-item array of the fact, the item converted by the schema's `items.type` by these same rules |
| anything else | the tool's session refuses to start, with an error naming the tool and ARG |

`--hide-tool NAME.TOOL` drops relay NAME's tool TOOL (its name on the relay)
from the listing, and a call to it is refused with an error saying it is hidden.
Use it for tools that change what every Box on the machine shares.

The runner keeps one MCP session per Box and relay. It starts on the Box's first
call of a relayed tool or tool listing, after prepare, as
`ssh <target> 'cd ~/moor/<box id> && exec "$SHELL" -lc CMD'` with `MOOR_BOX`,
`MOOR_SLOT`, `MOOR_PORT_OFFSET` and the `MOOR_FACT_*` variables set. It ends
when the sweep releases the Box (before release runs), after
`--relay-idle-secs` without a call, or when the server exits. Ending it closes
the server's stdin. A later call starts it again.

For the iOS example above, each Box drives only its own simulator through
Argent:

```sh
moor-machine-runner --socket /path/to/moor.sock \
  --state-dir ~/.local/state/moor-machine-runner \
  --prepare '~/bin/moor-prepare' --release '~/bin/moor-release' \
  --relay argent='argent mcp' \
  --force-arg argent.udid=device --force-arg argent.device_id=device \
  --force-arg argent.device=device --force-arg argent.devices=device \
  --force-arg argent.port=metro_port \
  --hide-tool argent.update-argent --hide-tool argent.dismiss-update
```

Argent names the device `udid`, `device_id`, `device` or `devices` (an array)
depending on the tool, and a per-Box `port` is an integer, so the example forces
each. Prepare prints both facts as
`{"device": "<udid>", "metro_port": "<8081 + MOOR_PORT_OFFSET>"}`. The Box sees
`argent_tap` and the rest without those arguments, each call reaches the
simulator prepare made for that Box, and `update-argent` and `dismiss-update`,
which change the install every Box shares, are not offered.
