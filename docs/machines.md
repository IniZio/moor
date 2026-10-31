# Machines

A machine is one of a user's own computers that a Box can use. The machine runs
`moor-machine-agent`, which dials out to the host and serves the Box's calls
over that link. The host never logs in to the machine, and the Box never
reaches it directly. See [Enrolling a machine](#enrolling-a-machine).

## Machine runner

`moor-machine-runner` is an MCP server on the host. An agent in a Box reaches it
at `http://machine.moor.internal/mcp`. The Box's Proxy routes that host to the
runner and sets four headers on every request. The runner trusts these headers
to name the caller.

| Header | Value |
| --- | --- |
| `X-Moor-Box` | the Box's numeric store id |
| `X-Moor-Machine` | the attached machine's name |
| `X-Moor-Machine-Id` | the machine's id from enrollment |
| `X-Moor-Machine-Slot` | the Box's slot on the machine, 0 to 6553 |

If a request misses a header or has a malformed one, it gets an MCP error that
names the header.

A new Box holds no machine. A start never claims one, even when the repo's
`.moor.yaml` has a `machine:` section. The Box claims one of its user's machines
itself with the runner's `machine_claim` tool, and the runner asks the
controller for it with [`machine-hold`](config.md#who-may-act-on-a-box). See
[Claiming a machine](#claiming-a-machine).

A Box holds no slot until it claims one, when its repo asks for no machine,
when it sleeps, or when it woke and waits for a free slot. The Proxy still passes its requests through,
with `X-Moor-Box` and `X-Moor-Machine-Missing: <reason>` in place of the
machine headers. The agent still sees the runner's own tools in the tool list,
and the list leaves out relayed tools. Every call except `machine_claim` and
`machine_release` returns an error result that gives the reason and says "Try
again later." Nothing runs on the machine. The runner does not refuse these requests, because
a refusal makes the agent's client ask for OAuth and drop the tools for the
rest of the session.

Several Boxes share a machine, one per slot. Every port of slot k is the repo's
port plus 10 times k. The port number is the same on the machine and in the
Box.

A Box that sleeps frees its slot for other Boxes. It keeps its claim on the
same machine, and its files there stay (`machine_detached` with
`asleep: true`). When it wakes, it joins the line for that machine and takes
the lowest free slot once the Boxes that asked before it have theirs. That slot
can differ from the old one, and so can its ports (`machine_attached` names
them). If every slot is taken, the Box runs without the machine, and
`machine_waiting` says why and the Box's `position` in line (counting from 1),
and says so again only when the position changes. The Proxy then tries again on
each tick until a slot frees or the Box sleeps or closes. Until then,
`moor show` names the machine as asleep or waiting, with the place in line. Closing the Box ends its
claim.

Start the runner next to the controller, as the same host user:

```sh
moor-machine-runner --socket <state dir>/runner/runner.sock \
  --state-dir ~/.local/state/moor-machine-runner --agents 0.0.0.0:7701
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--socket` | `$MOOR_RUNNER_SOCKET` | The controller's socket for the runner, `<state dir>/runner/runner.sock`. It serves only the requests the runner sends: `show`, `machine-hold`, `machine-keys`, `machine-links`, `box-mirror` and `box-put`. The runner needs no access to the controller's own socket. |
| `--state-dir` | | Where the runner keeps one host mirror and one record for each Box. |
| `--listen` | `127.0.0.1:7700` | The address the Proxy routes to. |
| `--agents ADDR` | none | Where machine agents dial in, at `/machines/link`. Without it, no machine can be reached. |
| `--service-host` | `machine.moor.internal` | The `Host` the Proxy passes through. The runner accepts it in addition to the loopback names. |
| `--resource NAME=CAP` | none | A capped resource that each machine has, such as `build=1`. Repeat the flag for more. |
| `--min-free-gb` | `20` | The free GB a machine needs before the runner prepares or syncs a Box there. Below it, the runner removes sleeping Boxes' files, oldest first. 0 turns it off. See [Disk guard](#disk-guard). |
| `--hook-timeout-secs` | `600` | How long `prepare`, `suspend`, or `release` can run. |
| `--sweep-secs` | `60` | How often the runner looks for prepared Boxes that are no longer open, and for idle relay sessions. |
| `--relay NAME` | none | A stdio MCP server that the machine's own relay file `NAME` starts. The runner lists its tools as `NAME_<tool>`. Repeat the flag for more. See [Relays](#relays). |
| `--force-arg NAME.ARG=FACT` | none | On every call to a tool of relay NAME, sets argument ARG to the Box's fact FACT and hides ARG from the Box. Repeat the flag for more. |
| `--hide-tool NAME.TOOL` | none | Leaves the tool TOOL of relay NAME out of the listing and refuses calls to it. Repeat the flag for more. |
| `--relay-idle-secs` | `1800` | How long a relay session can go without a call before it ends. |
| `--keep-alive-secs` | `15` | How often an open call's response stream gets a ping. At the same interval, a running `sync` or `exec` sends a progress notification (`sync: running for N s`) when the call asked for progress. This keeps the agent's MCP client and any proxy between them from seeing a long call as idle. |

A resource caps heavy work on a machine, such as native builds, at CAP calls at
once. The runner counts a resource for each machine name across all Boxes.
Only the runner process tracks holds, so a runner restart frees them all. With
no `--resource` flags, the runner holds nothing.

The runner stores and passes no secret. It reaches each machine only through
the link its agent opened.

## Seeing a machine's state

`moor user machine ls` prints one line per machine of yours: its name, whether
its agent is linked (`online`, `offline`, or `not linked` before it first
links) and the agent's version.

```
amd  online  1.2.3
old  offline  1.2.0
mac  not linked  -
```

`moor user machine status NAME` asks that machine's agent for its version, OS,
CPU architecture and free disk. A machine that hosts a Box pool also reports
its provider, its crabbox version and its leases, one `slug state` line each.
Last come the agent's last 50 operations, newest first, each with its time,
name, target and whether it succeeded. Add `--json` for the same data as JSON.

The request is read-only and never runs a command on the machine. It refuses
a machine that is offline, naming since when, and it refuses a name that is
not one of yours.

## Claiming a machine

An agent in a Box takes a slot on one of its user's machines with
`machine_claim`, and gives it back with `machine_release`. Both work while the
Box holds no machine.

`machine_claim` takes these optional arguments:

- `labels`, the labels the machine must all have
- `machine`, the name of the one machine wanted
- `ports`, the Box ports to forward
- `wait_secs`, how long to wait for a free slot (default 0, at most 3600)

Left out, `labels` and `ports` are the repo's `machine:` section. The call
returns `machine`, `slot`, `port_offset`, `ports`, and `facts`, as
`machine_info` gives them. A claim while the Box holds a slot returns that
slot. From the next request on, the Proxy sends the machine headers, so
`sync` and `exec` work without a restart.

With no free slot and `wait_secs` 0, the call fails at once with the reason,
such as `no free slot: machine mac is attached to Boxes a, b`. With
`wait_secs`, the Box joins the line. The runner asks again about every 2
seconds. While the call waits, it sends MCP progress notifications, such as
`waiting for mac: place 1 in line`, when the call asked for progress. When the
wait runs out, or the client cancels or hangs up, the runner gives up the
Box's place in line, and the call fails with the reason. A waiting claim
records `machine_waiting` with its place in line, again only when the place
changes, and a refused one records `machine_refused` with the reason;
`moor show` reports the place while the claim waits.

After a claim is held, the runner sends `notifications/tools/list_changed`. The
client then lists the tools again and sees the machine's relayed tools, which
the list leaves out while the Box holds no machine.

`machine_release` takes no arguments. It runs the machine's `suspend` hook for the
Box first, as when the Box falls asleep, and then gives the slot back. The
Box's files on the machine stay until the Box closes, so a later claim finds
them. It returns `{"released": "mac"}`, or `{"released": null}` when the Box
held no machine.

## Tools

`sync` copies the Box's work tree to `~/moor/<box id>` on the machine in two
hops. First, the runner asks the controller for a
[`box-mirror`](cli.md#box-mirror) into `<state dir>/mirrors/<box id>` on the
host. Then it runs `rsync -a --delete --exclude=.git` over the link from that
mirror to the machine. Both hops send only what changed since the last sync. Files
that are gone from the Box also go from the machine. The hops leave out `.git`
and every pattern in the Box's [sync excludes](config.md#sync-excludes). The
Proxy sends those patterns in `X-Moor-Machine-Exclude`. By default they leave
out `node_modules`, `Pods`, `.gradle`, `DerivedData`, `.dart_tool`, and
`target`, so install packages on the machine itself. `machine_info` returns the
list as `sync_exclude`.

`sync` takes an optional `exclude`. It is a list of up to 64 more rsync
patterns relative to the work tree root, such as `["app/android/app/build"]`. A
call adds to the Box's sync excludes and cannot remove one. Each pattern
becomes one `--exclude=PATTERN` argument of both hops. The Box then never sends
those paths to the host mirror. The machine neither receives them nor loses
them to `--delete`. Use `exclude` for what the machine builds for itself, such
as `node_modules` installed there. Otherwise a Box on another platform
overwrites it. The runner refuses, by name, a pattern that is empty, starts
with `-`, or holds a newline or NUL. The first hop is unchanged.

`pull` copies one file from the machine into the Box's work tree. Use it for
something the machine made, such as a screenshot that a relayed tool saved, or
a build log. `path` is absolute on the machine or relative to
`~/moor/<box id>`. `dest` is relative to the work tree. It cannot be empty or
absolute, or contain `..`. The runner copies the file to a staging file on the
host over the link. Then it asks the controller to write the file with
[`box-put`](cli.md#box-put). The file can be at most 32 MiB. The file replaces
an existing file at `dest`. `pull` returns `dest` and `bytes`.

`exec` takes `command` and `timeout_secs` (default 600, at most 3600). It runs
`<command>` in `~/moor/<box id>` on the machine through the machine user's
login shell, so that user's `PATH` applies. The machine's agent runs it only
when it was started with `--allow-exec`. It returns these fields:

- `exit_code`
- `output`, the last 64 KiB of combined stdout and stderr
- `truncated`, when the runner cut earlier output
- `timed_out`

When a command runs past its timeout, the agent kills it. The call
returns `timed_out: true` and no exit code.

`exec` also takes `hold`, a list of resource names, and `hold_wait_secs`
(default 900, at most 3600). Before the command runs, the call takes one unit
of each named resource on its machine. It takes them in name order, so two
calls never deadlock. The runner refuses an unknown name at once. While the
call waits, it sends MCP progress notifications, such as
`waiting for build on minion: held by Box 12`. If the wait runs out, the call
returns an error with that reason and runs nothing. A hold ends when the call
returns or times out, or when its client hangs up. So a call that drops while
it waits never takes the hold.

`machine_info` returns `box`, `machine`, `slot`, `port_offset`, and `facts`.
`port_offset` is 10 times the slot. An agent runs its dev servers on the
machine on the repo's port plus `port_offset`.

## Per-Box setup

Boxes that share a machine can each need their own things on it, such as their
own simulator device or build folder, removed when the Box closes. The
machine's owner supplies hooks for this: executable files named `prepare`,
`suspend` and `release` in `~/.config/moor-machine-agent/hooks/` on the machine
(under `$XDG_CONFIG_HOME` when it is set). The runner does not know what they
make, and a missing hook is skipped.

`prepare` runs before a Box's first `exec` or `machine_info`. The agent runs it
through the machine user's login shell. It
runs in `~/moor/<box id>`, which the runner makes if it is missing. These
variables are set:

| Variable | Value |
| --- | --- |
| `MOOR_BOX` | the Box's id |
| `MOOR_SLOT` | the Box's slot |
| `MOOR_PORT_OFFSET` | 10 times the slot |
| `MOOR_STATE_DIR` | the Box's state dir, `~/moor-state/<box id>`, as an absolute path |
| `TMPDIR` | `$MOOR_STATE_DIR/tmp` |

The runner makes the state dir and its `tmp` before it runs anything for the
Box. It gives the same two variables to `exec` commands and to the processes
that the relays start. So what a tool writes to its temp dir lands under the
Box's state dir, and goes when the Box does.

The last line that CMD prints on stdout must be a JSON object of string values.
These are the Box's facts, such as `{"device": "<udid>"}`. CMD can print
anything on stderr. CMD fails if it exits non-zero, runs past
`--hook-timeout-secs`, or ends on another line. Then the call that needed it
fails with the tail of its output, and the runner keeps nothing. The Box's next
call runs CMD again. Two calls of one Box never run it at the same time.

The runner keeps the facts, with the Box's machine and slot, in
`<state dir>/boxes/<box id>.json`. So a restarted runner does not prepare the
Box again. `machine_info` returns the facts as `facts`. Every `exec` command
sees each fact as `MOOR_FACT_<KEY>`. The key is in uppercase, and every
character that is not a letter or a digit becomes `_`. So `device` is
`MOOR_FACT_DEVICE`. With no `prepare` hook, the facts are empty and the runner
still keeps the record.

`release` runs after a prepared Box is no longer open. It runs in the
same place as `prepare`, with the same variables and the `MOOR_FACT_*`
variables. A `sync` also leaves a record, so the runner cleans up a Box that
only synced too. Every `--sweep-secs`, the runner asks the controller for the
open Boxes, with a `show` with no Box over the same socket. For each recorded
Box that the controller does not list, the runner does these steps:

1. Runs release, if the Box was prepared.
2. Removes `~/moor/<box id>` and `~/moor-state/<box id>` on the machine.
3. Deletes the Box's record and its host mirror `<state dir>/mirrors/<box id>`.

If a step fails, the runner keeps the rest, prints the tail of the output to
stderr, and tries again on the next sweep. If the runner cannot ask the
controller, it releases nothing.

Besides that, every sweep and right after an agent links, the runner sends each
linked ordinary machine the open Boxes, and the agent removes every other
`~/moor/<box id>` and `~/moor-state/<box id>` it finds, after that Box's
`release` hook (which sees `MOOR_BOX` only). It does this with or without
`--allow-exec`, never follows a symlink, and skips names that are not plain
ids and Boxes the runner has a record for. If the runner cannot list the open
Boxes, it sends nothing. A Box whose release fails keeps its folders until the
next sweep.

Removing a Box's files and reading the machine's free space are commands the
runner sends with `exec`, so a machine that Boxes claim runs its agent with
`--allow-exec`. Files of a Box that the runner has no record of, such as after
its state dir was lost, stay on the machine until someone removes them.

`sync` runs `rsync --delete` into `~/moor/<box id>`, so the next sync removes
anything that prepare puts there. Put per-Box folders, such as a build's
derived data, under `$MOOR_STATE_DIR` instead. The runner removes that
directory with the Box, so release has to undo only what lives elsewhere.

This example is for iOS work, where each Box gets its own simulator. The hooks
live on the Mac, in `~/.config/moor-machine-agent/hooks/`.

`prepare`:

```sh
#!/bin/sh
set -e
mkdir -p "$MOOR_STATE_DIR"/DerivedData
udid=$(xcrun simctl list devices | sed -n "s/.*moor-$MOOR_BOX (\([0-9A-F-]*\)).*/\1/p" | head -1)
[ -n "$udid" ] || udid=$(xcrun simctl create "moor-$MOOR_BOX" "iPhone 16" "iOS18.5")
printf '{"device": "%s", "metro_port": "%s"}\n' "$udid" "$((8081 + MOOR_PORT_OFFSET))"
```

`suspend`:

```sh
#!/bin/sh
xcrun simctl shutdown "$MOOR_FACT_DEVICE" 2>/dev/null
exit 0
```

`release`:

```sh
#!/bin/sh
[ -n "$MOOR_FACT_DEVICE" ] && xcrun simctl delete "$MOOR_FACT_DEVICE" 2>/dev/null
exit 0
```

An agent in the Box then builds with
`xcodebuild -destination "id=$MOOR_FACT_DEVICE" -derivedDataPath $MOOR_STATE_DIR/DerivedData`.

### Sleeping Boxes

A Box that the controller lists as asleep (`lifecycle` `asleep` in the `show`
reply) frees its slot. Its processes on the machine should stop, but its files
stay. On each sweep, the runner looks for recorded, prepared Boxes that are
asleep and not yet suspended. For each one, the runner ends the Box's relay
sessions and frees the capped resources that its calls hold, even for a call
that still runs. Then it runs the `suspend` hook, where `release` runs,
with the same variables, `MOOR_FACT_*` included, and the slot the Box had. Use
it to stop what `prepare` started, such as a simulator. Only after the hook
succeeds does the runner write the time in the Box's record as `suspended_at`.
The runner logs a failing suspend and runs it again on the next sweep. With no
`suspend` hook, the runner still marks the Box, so the steps below apply.

When the Box wakes, it can get a different slot, because the controller gives
it the lowest free slot on the same machine. The Box's next `exec`,
`machine_info`, relay call, or `sync` runs `prepare` again, with the slot from
the request. This happens if the Box was suspended, or if the request's slot
differs from the record's. The new facts and slot replace the old ones, and the
runner clears `suspended_at`. `machine_info` reports the new slot,
`port_offset`, and facts. So `prepare` must be idempotent. When it runs again
for a Box that already has its device and folders, it must reuse them. It must
also print the facts for the current slot, such as a Metro port that depends
on `MOOR_PORT_OFFSET`.

### Disk guard

`--min-free-gb N` (default 20, and 0 turns it off) keeps the machine from
filling up. Before it runs `prepare` for a Box, and before every `sync`, the
runner reads the free space in the machine user's home with `df -Pk ~`, sent
with `exec`. If the free space is below the floor, the runner takes the
machine's suspended Boxes that the controller still lists as asleep, oldest
`suspended_at` first. For each one, it runs `release`, removes
`~/moor/<box id>` and `~/moor-state/<box id>`, and reads the free space again.
It stops once the free space is above the floor. The runner keeps the Box's
record and its host mirror. It marks the record `evicted`, clears its facts,
and sets `prepared` to false. It never touches a Box that is awake, a Box in
the middle of a call, or anything outside the Boxes' own directories, such as
shared caches. If the space is still below the floor and no sleeping Box is
left, the call fails. The error gives the free GB, the floor, and that no
sleeping Box is left to evict.

An evicted Box's next call prepares it again. Until the Box syncs,
`machine_info` carries a `notice`. The notice says that the runner removed the
Box's files on the machine while it slept, so the Box must `sync` again.

## Relays

A machine can run its own MCP servers over stdio, such as Argent
(`argent mcp`) for driving simulators. The machine's owner puts an executable
file `relays/NAME` in its hooks dir that starts the server, such as
`exec argent mcp`. `--relay NAME` lists that server's
tools to every Box, next to `sync`, `exec`, and `machine_info`. NAME matches
`[a-z][a-z0-9-]*`.

The runner lists each tool as `NAME_<tool>`, and turns every character that is
not a letter, a digit, or `_` into `_`. For example, relay `argent`'s
`list-devices` is `argent_list_devices`. Two tools can get the same name, or
one can take a name the runner already lists. Then that relay's session refuses
to start and the runner leaves out its tools. The reason goes to stderr and
into the error of a call to one of those tools. Results, errors, and progress
come back from the server unchanged.

`--force-arg NAME.ARG=FACT` keeps a Box on what was set up for it. For each
tool of relay NAME whose input schema has ARG, the runner removes ARG from the
schema's `properties` and `required`, so the agent never sees it. On every
call, the runner sets ARG to the Box's fact FACT and replaces anything the
agent sent. A Box without FACT gets an error that names the fact, and the
runner does not relay the call.

Facts are strings, so the value that the runner sends follows the type that the
tool's schema gives ARG:

| Schema type | Sent |
| --- | --- |
| `string`, or none | The fact as is. |
| `integer`, `number` | The fact parsed as a JSON number. If the fact does not parse, the call gets an error that names the fact and the tool, and the runner does not relay it. |
| `array` | A one-item array of the fact. The runner converts the item by the schema's `items.type`, with these same rules. |
| anything else | The tool's session refuses to start, with an error that names the tool and ARG. |

`--hide-tool NAME.TOOL` drops the tool TOOL of relay NAME from the listing.
TOOL is the tool's name on the relay. The runner refuses a call to it with an
error that says it is hidden. Use it for tools that change what every Box on
the machine shares.

The runner keeps one MCP session for each Box and relay. The session starts on
the Box's first call of a relayed tool or tool listing, after prepare. The
agent runs the relay file in `~/moor/<box id>` through the login shell. `MOOR_BOX`,
`MOOR_SLOT`, `MOOR_PORT_OFFSET`, and the `MOOR_FACT_*` variables are set. The
session ends when the sweep releases the Box, before release runs. It also ends
after `--relay-idle-secs` without a call, or when the server exits. Ending the
session closes the server's stdin. A later call starts it again.

For the iOS example above, each Box drives only its own simulator through
Argent:

```sh
moor-machine-runner --socket <state dir>/runner/runner.sock \
  --state-dir ~/.local/state/moor-machine-runner --agents 0.0.0.0:7701 \
  --relay argent \
  --force-arg argent.udid=device --force-arg argent.device_id=device \
  --force-arg argent.device=device --force-arg argent.devices=device \
  --force-arg argent.port=metro_port \
  --hide-tool argent.update-argent --hide-tool argent.dismiss-update
```

Argent names the device `udid`, `device_id`, `device`, or `devices` (an array),
depending on the tool. A per-Box `port` is an integer. So the example forces
each one. Prepare prints both facts as
`{"device": "<udid>", "metro_port": "<8081 + MOOR_PORT_OFFSET>"}`. The Box sees
`argent_tap` and the other tools without those arguments. Each call reaches the
simulator that prepare made for that Box. The runner does not offer
`update-argent` and `dismiss-update`, because they change the install that
every Box shares.

## Enrolling a machine

The machine's owner installs `moor-machine-agent` on it and enrolls it once.
Enrolling signs the owner in through the browser and makes a key that only
this machine holds:

```sh
moor-machine-agent enroll --server https://moor.example.com --name mac --labels macos,ios-sim
```

Then keep the agent running, for example as a launch agent or a systemd user
service:

```sh
moor-machine-agent run --allow-exec
```

The agent dials the runner and serves the Box's calls over that link. It runs
a command that the runner sends with `exec` only when it is started with
`--allow-exec`, so a controller that someone else takes over cannot run
commands on the machine unless its owner allowed that. `sync`, `pull`, the
hooks and the relays work without the flag, but the `exec` tool, the disk
guard and removing a closed Box's files need it. A pool host, started with
`--pool`, never runs `exec`, and the agent refuses `--allow-exec` with
`--pool`.
