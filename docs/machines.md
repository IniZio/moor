# Machine tools

A Box can borrow a tool machine that its user paired with `--allow-exec`. The
agent reaches the machine runner at `http://machine.moor.internal/mcp`, and the
Box's Proxy adds headers that name the Box and its machine. The runner reaches
the machine only over the link its agent opened.

## Tools

| Tool | Arguments | What it does |
| --- | --- | --- |
| `machine_claim` | optional `labels`, `machine`, `ports`, `wait_secs` | Takes a slot on one of the user's machines. Returns `machine`, `slot`, `port_offset`, `ports`, and `facts`. |
| `machine_release` | none | Runs the `suspend` hook and gives the slot back. The Box's files stay until it closes. |
| `sync` | optional `exclude` | Copies the work tree to `~/moor/<box id>` on the machine, sending only changes. |
| `pull` | `path`, `dest` | Copies one file, up to 32 MiB, from the machine into the work tree. |
| `exec` | `command`; optional `timeout_secs`, `hold`, `hold_wait_secs` | Runs a command in `~/moor/<box id>` through the machine user's login shell. |
| `machine_info` | none | Returns `box`, `machine`, `slot`, `port_offset`, and `facts`. |

- `machine_claim` with no `labels` or `ports` uses the repo's `machine:`
  section. With no free slot and `wait_secs` 0, it fails at once with the
  reason. With `wait_secs`, at most 3600, the Box waits its turn.
- Several Boxes share a machine, one per slot. Slot `k` has a `port_offset` of
  `10·k`, and each port `p` becomes `p + 10·k` on both the Box and the machine.
- `sync` leaves out `.git` and the Box's sync excludes. By default those are
  `node_modules`, `Pods`, `.gradle`, `DerivedData`, `.dart_tool`, and
  `target`, so install packages on the machine itself. `exclude` adds up to 64
  more patterns.
- `exec` runs for 600 seconds by default, 3600 at most. It returns
  `exit_code`, the last 64 KiB of `output`, `truncated`, and `timed_out`.
- `exec` with `hold` takes one unit of each named resource that the runner's
  `--resource NAME=CAP` caps, such as one native build at a time.
- A Box that sleeps frees its slot and keeps its claim. On waking, it takes the
  lowest free slot, which can change its ports.

## Hooks

Executable files in `~/.config/moor-machine-agent/hooks/` on the machine give
each Box its own things there, such as a simulator. A missing hook is skipped.

| Hook | Runs |
| --- | --- |
| `prepare` | Before a Box's first `exec` or `machine_info`, and again after the Box wakes on a new slot. |
| `suspend` | When the Box sleeps or releases its slot. |
| `release` | After the Box closes. |

Each hook runs in `~/moor/<box id>` through the login shell, within
`--hook-timeout-secs` (600 by default), with these variables:

| Variable | Value |
| --- | --- |
| `MOOR_BOX` | The Box's id. |
| `MOOR_SLOT` | The Box's slot. |
| `MOOR_PORT_OFFSET` | 10 times the slot. |
| `MOOR_STATE_DIR` | `~/moor-state/<box id>`, removed with the Box. |
| `TMPDIR` | `$MOOR_STATE_DIR/tmp`. |
| `MOOR_FACT_<KEY>` | Each fact `prepare` printed, as for `exec`. |

The last stdout line of `prepare` must be a JSON object of strings. Those are
the Box's facts, such as `{"device": "<udid>"}`. Every `exec` sees them as
`MOOR_FACT_<KEY>`. `prepare` must be safe to run again, and must print the
facts for the current slot.

Put per-Box folders under `$MOOR_STATE_DIR`. The next `sync` deletes anything
else `prepare` puts in `~/moor/<box id>`.

This iOS example gives each Box its own simulator. `prepare`:

```sh
#!/bin/sh
set -e
mkdir -p "$MOOR_STATE_DIR"/DerivedData
udid=$(xcrun simctl list devices | sed -n "s/.*moor-$MOOR_BOX (\([0-9A-F-]*\)).*/\1/p" | head -1)
[ -n "$udid" ] || udid=$(xcrun simctl create "moor-$MOOR_BOX" "iPhone 16" "iOS18.5")
printf '{"device": "%s", "metro_port": "%s"}\n' "$udid" "$((8081 + MOOR_PORT_OFFSET))"
```

`suspend` runs `xcrun simctl shutdown "$MOOR_FACT_DEVICE"`, and `release` runs
`xcrun simctl delete "$MOOR_FACT_DEVICE"`. The agent then builds with
`xcodebuild -destination "id=$MOOR_FACT_DEVICE"`.

## Relays

A machine can offer its own stdio MCP servers to Boxes. Put an executable
`relays/NAME` in the hooks directory that starts the server, such as
`exec argent mcp`. Then start the runner with `--relay NAME`. Each tool shows
to the Box as `NAME_<tool>`.

| Runner flag | Effect |
| --- | --- |
| `--relay NAME` | Offer relay `NAME`'s tools. |
| `--force-arg NAME.ARG=FACT` | Set argument `ARG` of relay `NAME`'s tools to the Box's fact `FACT`, and hide it from the Box. |
| `--hide-tool NAME.TOOL` | Leave a relay's tool out and refuse calls to it. |
| `--relay-idle-secs` | End a relay session after this long without a call. 1800 by default. |

So each Box drives only its own simulator through Argent:

```sh
moor-machine-runner --socket ~/.local/state/moor/runner/runner.sock \
  --state-dir ~/.local/state/moor-machine-runner --agents 0.0.0.0:7701 \
  --relay argent --force-arg argent.udid=device --force-arg argent.port=metro_port \
  --hide-tool argent.update-argent
```

## Disk guard

Before `prepare` and before each `sync`, the runner checks the machine's free
space. Below `--min-free-gb` (20 by default, 0 turns it off), it removes the
files of sleeping Boxes, oldest first, until there is room. It never touches an
awake Box. If no sleeping Box is left, the call fails with the free and needed
space. An evicted Box's next call prepares it again, and it must `sync` again.

`moor-machine-runner --help` lists the runner's other flags.
