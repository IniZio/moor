---
name: moor-boxes
description: Use when a line starting with "moor: box" or "moor-listen:" arrives from the moor monitor, or when asked to start, steer or check moor Boxes.
---

# Acting on moor Box events

The `moor-boxes` monitor prints one line per Box event. Never poll Boxes in a
loop; wait for the next line.

## Box lines

`moor: box <name> <event>: <first line of what the Box said>`

| Event | What to do |
| --- | --- |
| `idled`, `needs_input` | Read `moor://box/<name>` or call `box_status`. Act on it with `send_to_box` if the answer is clear, otherwise tell the user what the Box asks. |
| `setup_failed`, `session_failed`, `launch_timed_out`, `box_failed`, `relaunch_capped` | Report it to the user with the Box name and the line. Do not restart or close the Box unless asked. |
| `closed` | Mention it only if the user was waiting on that Box. |

## Listener lines

- `moor-listen: stream lost (...); reconnecting` and `moor-listen: reconnected`:
  nothing to do. Events during the gap may be missed, so call `list_boxes` once
  after `reconnected`.
- `moor-listen: not signed in to <origin>; run moor login --server <origin>`:
  tell the user to run that command once. The listener picks the sign-in up
  within a minute.
- `moor-listen: token rejected; ...`: tell the user to run
  `moor login --server <origin>` again.
- `moor-listen: moor is not installed` or `set MOOR_SERVER`: tell the user the
  one step the line names.
