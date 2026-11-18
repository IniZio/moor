# Run Boxes on a schedule

A workflow starts a Box in each of your repos on a cron schedule, each with the
same prompt. Use one for recurring work, such as fixing security alerts across
several repos every morning.

## Add a workflow

Ask Claude Code in plain words:

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
- `config`, optional, a partial repo config in YAML that every Box starts
  with, laid over the repo's own. See [Override for one
  Box](repo.md#override-for-one-box).
- `prompt`, what each Box's agent is asked to do.
- `timeout_minutes`, optional, 180 by default.
- `plan`, optional, the steps of the run. See [Give a run a plan](#give-a-run-a-plan).
- `expected_revision`, optional, the `revision` that `list_workflows` showed.
  The call is refused when the workflow changed after you read it.

A cron that can fire twice within an hour is refused.

Each run starts a Box as you, with your secrets and your repo access. Its
agent can push branches and open pull requests. It cannot merge them. The Box
is named `<repo>/w<id>-<workflow>-<due time>`, with the due time in UTC.

## Give a run a plan

Without a plan, a run is one step: the prompt. A plan splits the job into steps
that you review before they run. Your agent writes the plan in the same
`add_workflow` call, as a `steps` list:

- A `work` step is something the agent does. It names the evidence files the
  agent leaves in `.moor/artifacts/`, as flat names such as `report.md` or a
  prefix such as `before-*`.
- A `deliver` step is the last one: the pull request, message or file the
  prompt asks for. It ends the run, so it has no `ok` or `fail`.
- Every other step has an `ok` edge to the id of the next step, and may have a
  `fail` edge. Every step must lead on to a deliver step.
- `max_tries`, 1 to 5, caps how often a run may enter a step. A step entered
  more often follows its `fail` edge, or fails the run when it has none.

moor saves nothing until the plan is valid, and it lists every error at once.
The reply shows the stored plan as numbered steps next to the new `revision`.
Ask for changes in plain words, and the agent sends the plan again with that
revision. Keep the ids of steps that did not change, so a run's history stays
attached to them.

A run freezes the plan when it fires, so an edit never changes a run in
flight. The Box is first told the prompt, the number of steps and the first
step. When its agent goes quiet, moor sends it the next step, and the run is
done when it goes quiet at the last one. `list_workflows` shows the plan and
`plan_stale` when the prompt changed after the plan was made. A stale plan
still runs. Adding a workflow again without a `plan` removes its plan.

A `check` step is run by moor, never by the agent. moor waits until the agent
is quiet. Then it runs the command in the Box's work place. The deadline is the
smaller of the run's remaining timeout and 30 minutes. Exit 0
follows `ok`. Any other exit, or the deadline passing, follows `fail`. The last
4 KiB of output goes into the run's history, and the agent is steered with the
result and the next step's instructions. A check that has used its `max_tries`
follows its `fail` edge, and a run that cannot go on fails. A restart of the
machine agent can cut a check's stream. That is no result, so moor runs the
check again on the next pass, and a second cut in one try follows `fail`. A
check command must therefore be safe to run twice.

A work step whose agent crashed or exited is not passed. moor sends the step
again once. A second end without progress ends the run `incomplete`.

An `approve` step asks the workflow's owner its `question`. The run waits
(`waiting` in `list_workflows`) and its Box is left to sleep. The owner answers
with `answer_run(workflow, repo, answer, note)`. A `yes` follows `ok` and a
`no` follows `fail`. A `no` at a step with no `fail` edge ends the run
`declined`.
The next step goes to the agent as a steer, which wakes the Box, and the note
goes with it. Waiting does not count toward `timeout_minutes`. A wait with no
answer ends the run `declined` after 7 days, so the next firing is not skipped
forever. A waiting run blocks the next firing like a running one, and a
declined run neither counts toward the breaker nor resets it.

## Run a workflow now

Ask the agent to run a workflow once by hand, which calls
`run_workflow(name, repos, expected_revision)`. You do not have to wait for the
cron. It works on a disabled workflow too, so you can add one, disable it, run it
until its plan is right, and enable it after.

- `repos` defaults to every repo of the workflow. Each must be one of them.
- `expected_revision` refuses the call when the workflow changed after you read
  it, so the run tries the plan you reviewed.
- The reply has one line per repo. A line says `started` with the Box name, or
  `refused` with the reason.
- The reply comes once each run is recorded, while its Box sets up. Follow the
  Box with `box_status` and the run with `list_workflow_runs`. A Box start that
  fails after the reply shows there as a refused run.
- The Workflows page of the account portal has a **Run now** button per repo.

A run fired by hand is an ordinary run. It freezes the plan, starts a Box as you,
walks the steps and has the workflow's timeout. `list_workflow_runs` shows it
with `trigger` `manual`. Its Box is named
`<repo>/w<id>-<workflow>-m<UTC time to the second>`, so it never shares a name
with a cron run.

How it sits beside the schedule:

- A repo with an open run is refused with that run's Box, and nothing is
  recorded. A run is open while it starts, works or waits for an answer. It is
  also open after a timeout while its Box is open.
- A cron run that comes due while a manual run is open is skipped, as for any
  open run.
- A manual run never swallows a due time the schedule missed.
- A manual run that fails does not count toward the breaker. One that delivers
  resets it, so a fix you proved by hand clears the earlier failures.

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

When a Box's places die, moor does not resume the Box.
Its run fails at its step at once, with the reason `death_capped` gives.
The run does not follow the step's `fail` edge or try again, because either
would wake the Box. The Box stays open so you can look inside.

A missing secret or a full node can refuse a run. The refusal is recorded with
its reason, and the run is not retried. The next due time is the next try.

Three refused or timed-out runs in a row for one repo disable the workflow.
Runs you fired by hand do not count. `list_workflows` shows the reason. Ask to enable it again once you fix the
cause.

## Follow the runs

- `list_workflows` shows each workflow, its next due time, and its latest run
  per repo.
- `list_workflow_runs` lists each change oldest first, with an outcome of
  `starting`, `started`, `finished`, `timed_out`, `skipped_overlap`, or
  `refused`, and a `trigger` of `cron` or `manual`. Pass the reply's `next` as `after` to get only newer changes.

## Pause or remove a workflow

Ask to disable it, which calls `set_workflow_enabled`, or to remove it, which
calls `remove_workflow`.

A disabled workflow fires no more runs. A run already in flight still
finishes.
