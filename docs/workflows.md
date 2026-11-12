# Run Boxes on a schedule

A workflow starts a Box in each of your repos on a cron schedule, each with the
same prompt. Use one for recurring work, such as fixing security alerts across
several repos every morning.

## Add a workflow

Ask Claude Code, or the Slack coordinator, in plain words:

```text
Every weekday at 9:00 London time, start a Box on acme/web and acme/api to
fix open Dependabot alerts and open a PR for each fix.
```

The agent calls `add_workflow` with these values:

- `name`, unique among your workflows. Adding the same name again replaces it.
- `cron`, five fields. `0 9 * * 1-5` is 9:00 on weekdays.
- `timezone`, an IANA name such as `Europe/London`.
- `repos`, one or more `owner/name`. Each firing starts one Box per repo.
- `also`, optional, more `owner/name` repos that every Box clones beside its
  own, so its agent can read and push to them too. Each must be a repo you can
  push to, and no two repos of one Box may share a name. None by default: the
  controller's `box.also` never applies to a workflow.
- `prompt`, what each Box's agent is asked to do.
- `timeout_minutes`, optional, 180 by default.

A cron that can fire twice within an hour is refused.

Each run starts a Box as you, with your secrets and your repo access. Its
agent can push branches and open pull requests. It cannot merge them. The Box
is named `<repo>/w<id>-<workflow>-<due time>`, with the due time in UTC.

## Know when a run fires

- A new or re-enabled workflow fires only for due times after that moment.
- After downtime, moor fires the latest missed due time once.
- A run is skipped while the previous run for that repo is unfinished. A run
  is finished once its agent has been quiet for two minutes, has exited, or
  its Box is closed.

## Limits and the breaker

A run has one hour from the firing for its agent to start. A run that misses
that is refused, and its Box is closed. The run then has `timeout_minutes`
from the agent's start. A run still working after that is stopped. Its Box
stays open so you can look inside.

A missing secret or a full node can refuse a run. The refusal is recorded with
its reason, and the run is not retried. The next due time is the next try.

Three refused or timed-out runs in a row for one repo disable the workflow.
`list_workflows` shows the reason. Ask to enable it again once you fix the
cause.

## Follow the runs

- `list_workflows` shows each workflow, its next due time, and its latest run
  per repo.
- `list_workflow_runs` lists each change oldest first, with an outcome of
  `starting`, `started`, `finished`, `timed_out`, `skipped_overlap`, or
  `refused`. Pass the reply's `next` as `after` to get only newer changes.

A workflow added in a Slack thread reports to Slack. Each run posts a new
message in that channel, mentions you, and becomes the run's own thread. Reply
there to steer the Box.

## Pause or remove a workflow

Ask to disable it, which calls `set_workflow_enabled`, or to remove it, which
calls `remove_workflow`.
