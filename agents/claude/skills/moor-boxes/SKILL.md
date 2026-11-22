---
name: moor-boxes
description: Use when starting, steering or checking moor Boxes or workflow runs, when asked to watch a Box or a run, or when a line starting with "moor: box" or "moor: run" arrives from a Monitor.
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

To watch a workflow's runs, after `run_workflow` or when the user asks, start a
background Monitor running:

```sh
moor listen --server "${MOOR_SERVER:-$MOOR_URL}" --workflow <name>
```

Pass `--workflow` once per workflow, and add `--box` to watch Boxes in the same
Monitor. A workflow never closes, so this Monitor runs until you stop it. Never
call `list_workflow_runs` in a loop to follow a run; wait for the next line. To
find one run, pass `workflow` and `pull` to `list_workflow_runs`.

## Box lines

`moor: box <name> <event>: <first line of what the Box said>`

| Event | What to do |
| --- | --- |
| `idled`, `needs_input` | Read `moor://box/<name>` or call `box_status`. Act on it with `send_to_box` if the answer is clear, otherwise tell the user what the Box asks. |
| `setup_failed` | Report it to the user with the Box name and the line, and offer `retry_box` to run setup again or `close_box` to give it up. Do neither unless asked. When several Boxes failed setup, `close_failed_boxes` closes them all. |
| `session_failed`, `launch_timed_out`, `box_failed`, `relaunch_capped`, `death_capped` | Report it to the user with the Box name and the line. Do not restart or close the Box unless asked. |
| `closed` | Mention it only if the user was waiting on that Box. |

## Run lines

`moor: run <workflow> <run id> <event>: <detail> (<repo>#<n>@<sha7>)`

The detail is the first line of the step title, trigger, result or reason. The
pull request part is there only for a run on a pull request.

| Event | Detail | What to do |
| --- | --- | --- |
| `started` | What fired it: `manual`, `cron` or `pull_request` | Nothing; note the run id. |
| `step` | The step's title | Nothing, unless the user asked to follow each step. |
| `waiting` | The approve step's title | Tell the user what the run asks. Call `answer_run` only when they answer. |
| `ended:finished` | The run's result | Tell the user it finished, with the result. |
| `ended:failed`, `ended:incomplete`, `ended:timed_out`, `ended:refused` | Why it ended | Report it with the run id and the line. Do not rerun it unless asked. |
| `ended:declined`, `ended:cancelled`, `ended:skipped_overlap`, `ended:skipped_fork` | Why, when moor knows | Mention it only if the user was waiting on that run. |

## Listener lines

- `moor-listen: not signed in to <origin>` or `moor-listen: token rejected`: tell
  the user to run `moor login --server <origin>`.
- `moor-listen: no Box named <name> is visible`: check the name with `list_boxes`.
