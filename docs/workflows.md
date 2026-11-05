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

A run still working after `timeout_minutes` is stopped. Moor ends its agent
session and keeps the Box open, so you can inspect it. The run is recorded as
timed out. The clock starts when the Box starts, so time spent queued for a
machine counts.

Three refused or timed-out runs in a row for one repo disable the workflow. A
skipped run neither counts nor resets the streak. A finished run resets it.
`list_workflows` shows the reason. `set_workflow_enabled` with `enabled: true`
clears the reason and restarts the count.

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
