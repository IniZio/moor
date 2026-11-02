---
name: moor-boxes
description: Use when starting, steering or checking moor Boxes, when asked to watch a Box, or when a line starting with "moor: box" arrives from a Box Monitor.
---

# Watching moor Boxes

Nothing listens until you ask it to. After `start_box` succeeds, or when the
user asks you to watch an existing Box, start a background Monitor running:

```sh
moor listen --server "${MOOR_SERVER:-$MOOR_URL}" --box <name>
```

Pass `--box` once per Box to watch several in one Monitor, or run one Monitor per
Box. If neither variable is set, ask the user for the moor server URL. The
Monitor ends by itself once every named Box has closed. Never poll Boxes in a
loop; wait for the next line.

## Box lines

`moor: box <name> <event>: <first line of what the Box said>`

| Event | What to do |
| --- | --- |
| `idled`, `needs_input` | Read `moor://box/<name>` or call `box_status`. Act on it with `send_to_box` if the answer is clear, otherwise tell the user what the Box asks. |
| `setup_failed` | Report it to the user with the Box name and the line, and offer `retry_box` to run setup again or `close_box` to give it up. Do neither unless asked. When several Boxes failed setup, `close_failed_boxes` closes them all. |
| `session_failed`, `launch_timed_out`, `box_failed`, `relaunch_capped` | Report it to the user with the Box name and the line. Do not restart or close the Box unless asked. |
| `closed` | Mention it only if the user was waiting on that Box. |

## Listener lines

- `moor-listen: not signed in to <origin>` or `moor-listen: token rejected`: tell
  the user to run `moor login --server <origin>`.
- `moor-listen: no Box named <name> is visible`: check the name with `list_boxes`.
