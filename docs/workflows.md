# Workflows

A workflow starts a Box in each of your repos on a cron schedule. Each Box gets
the same prompt. Use one for a daily job, such as fixing security alerts across
several repos.

You manage workflows through the [MCP tools](mcp.md#tools). Each tool acts as
the person who signed in, so you see and change only your own workflows.

## Add a workflow

`add_workflow` takes these arguments.

| Argument | Meaning |
| --- | --- |
| `name` | Unique among your workflows. Lowercase letters, digits, `.`, `_` and `-`, up to 64. Adding a name again replaces that workflow. |
| `cron` | Five fields: minute, hour, day of month, month, day of week. |
| `timezone` | An IANA name such as `Europe/London`. The cron is read in this zone. |
| `repos` | One or more `owner/name`. Each firing starts one Box per repo. |
| `prompt` | What each Box's agent is asked to do. |
| `timeout_minutes` | Optional. 1 to 720. Default 180. |

A refused add names the field that is wrong, with the bad cron or zone in the
message.

A cron that can fire twice within an hour is refused. The controller sets that
floor with [`workflow_min_interval_secs`](config.md#controller-config).

## What a run does

A run starts a Box as you. It uses your secrets, your agent profile and your
repo access, the same as a Box you start yourself. Its agent may push branches
and open pull requests in the repo. It cannot merge them.

A Box is named `<repo>/w<id>-<workflow>-<due time>`. The due time is UTC,
`yyyymmddhhmm`.

A run that moor cannot start is recorded with the refusal message. A missing
secret, a full machine and a repo you cannot read all refuse this way. Moor
does not retry a refused run. The next due time is the next try.

## When it fires

- A new or re-enabled workflow fires only for due times after that moment.
- After downtime, moor fires only the latest missed due time, once.
- Daylight saving follows the cron library. A time inside a spring gap fires
  once just after the gap. A time repeated in the autumn fires once, at its
  first pass.
- A run is skipped while the previous run for that repo is unfinished. The skip
  is recorded.

A run is unfinished while its Box is starting, while its agent works, and after
a timeout while its Box is still open. A run is finished once its agent has
been quiet for two minutes, has exited, or its Box is closed. An agent waiting
on background work is still working. A finished run never blocks the next one,
even when its Box stays open.

## Timeouts and the breaker

A run has two time limits.

The start limit is one hour, from the firing until the run's agent has started.
It covers waiting for a machine and setting the Box up. A run whose agent has
not started by then is refused, and its Box is closed so the agent can never
start later. If the Box is still being set up, that setup is stopped. The run
counts as a failed start, and the next firing is not held back.

The run limit is `timeout_minutes`, counted from when the agent started, so slow
setup does not use it up. A run still working after that is stopped: Moor ends
its agent session and keeps the Box open, so you can inspect it. The run is
recorded as timed out. After a controller restart both limits keep counting from
the same moments, the firing and the agent's start.

Three refused or timed-out runs in a row for one repo disable the workflow. A
skipped run neither counts nor resets the streak. A finished run resets it.
`list_workflows` shows the reason. `set_workflow_enabled` with `enabled: true`
clears the reason and restarts the count.

## In Slack

A workflow added in a Slack thread reports back to Slack. When a run fires, a
new top-level message appears in the thread's channel, before its Box has
finished setting up. It mentions the person who owns the workflow, names the
workflow and repo, and links the thread that added the workflow. That message is
a thread of its own. Once the Box exists, it works there: its status, artifacts,
questions and the agent's final message all land in it. The owner can reply in
it to steer the Box, like in any thread. A run that is refused after it fired
says why in that thread, and a run that timed out says so there.

Nothing else about a run goes in the thread that added the workflow. It hears
only runs that were skipped or refused before they fired, once per streak for
each repo, and a workflow the controller disabled.

## See what happened

`list_workflows` shows each workflow with its next due time, its latest run per
repo and any disabled reason.

`list_workflow_runs` returns changes oldest first. A change is a run or a
disabling by the controller. A run appears again each time it moves on, from
started to finished or timed out. Each entry has a `seq`. Pass the reply's
`next` as `after` to get only newer changes. Skips and refusals appear too.

| Outcome | Meaning |
| --- | --- |
| `starting` | Claimed, its Box is being started. |
| `started` | Its Box runs. |
| `finished` | The agent finished. |
| `timed_out` | Stopped at the timeout. |
| `skipped_overlap` | The previous run was unfinished. |
| `refused` | The start was refused. `detail` says why. |

A run that is starting or was refused also has `planned_box`, the name its Box
has or would have. `box` appears only once that Box exists.
