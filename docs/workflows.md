# Workflows for teams

A workflow runs a task on a schedule, such as fixing security alerts every
weekday morning. Each firing is a **run**, and each run is one task in one Box
with the workflow's prompt and repos. Open **Workflows** from your account menu.

![The Workflows page with one workflow and Run now](images/workflows.png)

Each row shows the schedule in words, the repos, the last run and the next one.

## Make a workflow

Choose **New workflow** and fill in the form.

![The New workflow form while Generate plan drafts a plan](images/workflow-form.png)

- **Name.** Lowercase letters, digits, dots, dashes and underscores. A workflow
  keeps its name.
- **Repos.** One or more, first one first. Each run clones them all into one
  Box. The agent works in the first, and its branch and pull request go there. A
  repo it changes gets its own branch and pull request. Each must be a repo you
  can push to.
- **Prompt.** What each run's agent is asked to do.
- **Plan.** Optional steps for each run. See [Give a run a plan](#give-a-run-a-plan).
- **Schedule.** Every weekday, every day, every week on a day, or every few
  hours, at a time in a time zone. **Advanced (cron)** takes five cron fields.
  The form shows the next three runs. A schedule that can fire twice within an
  hour is refused.
- **More.** The timeout. A run that takes longer is stopped. Three hours when
  blank.

To run the same schedule on separate projects, make one workflow per project.
**Duplicate** on a workflow copies it, plan included.

Each run starts as you, with your secrets and your repo access. Its agent can
push branches and open pull requests. It cannot merge them.

### Make this recurring

A finished task you want again has **Make this recurring** in the right rail of
its Task page. It opens the form with that task's repos and first prompt. Add a
schedule and save.

## Give a run a plan

Without a plan, a run is one step, the prompt. A plan splits the job into
numbered steps that you review before they run.

![A plan of four steps in the plan editor](images/workflow-plan.png)

- **Generate plan** starts a short task that reads the repos and the prompt and
  drafts the steps. It changes nothing in the repos. The draft lands in the
  editor, and nothing is saved until you save.
- **Add a plan** starts from one deliver step.
- Each step has **Edit**, move up, move down, **Remove** and **Add a step
  after**. Pick where ok and fail lead by the step's title.
- **Edit as JSON** shows the same plan as text.

There are four kinds of step.

| Kind | What it does |
| --- | --- |
| **work** | Something the agent does. It names the evidence files it leaves in `.moor/artifacts/`. |
| **check** | A command moor runs in the Box when the agent is quiet. Exit 0 passes. |
| **approve** | A question for the workflow's owner. The run waits for **Approve** or **Reject**. |
| **deliver** | The last step, the pull request, message or file the prompt asks for. It ends the run. |

Every step but deliver leads on to a next step when it passes, and may lead
elsewhere when it fails. **Tries** caps how often a run may enter a step, from 1
to 5. moor saves nothing until the plan is valid. It lists every error at once.

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
with `answer_run(workflow, answer, note)`. A `yes` follows `ok` and a
`no` follows `fail`. A `no` at a step with no `fail` edge ends the run
`declined`.
The next step goes to the agent as a steer, which wakes the Box, and the note
goes with it. Waiting does not count toward `timeout_minutes`. A wait with no
answer ends the run `declined` after 7 days, so the next firing is not skipped
forever. A waiting run blocks the next firing like a running one, and a
declined run neither counts toward the breaker nor resets it.

A saved prompt that changed after the plan was made marks the plan **plan
stale**. It still runs. **Mark reviewed** saves it as made from the current
prompt.

## Run now

**Run now** starts a run at once, so you can try a workflow before its schedule
fires. It works on a disabled workflow too. Make one, disable it, run it until
its plan is right, then enable it.

A run fired by hand is an ordinary run. It freezes the plan, starts a Box as
you, walks the steps and has the workflow's timeout. It opens the same Task
page as any task, with the plan in the right rail.

How it sits beside the schedule:

- A workflow with an open run is refused with that run's Box, and nothing is
  recorded. A run is open while it starts, works or waits for an answer. It is
  also open after a timeout while its Box is open.
- A cron run that comes due while a manual run is open is skipped, as for any
  open run.
- A manual run never swallows a due time the schedule missed.
- A manual run that fails does not count toward the breaker. One that delivers
  resets it, so a fix you proved by hand clears the earlier failures.

## Follow runs and retry

The workflow's page lists its runs, with how each started, when, how long it
took and how it ended. A run opens its step history. A run that waits at an approve step puts its task in **Needs you**,
and the card asks the step's question.

A failed run's task shows **Run again**, which starts a new run of the
workflow. A setup that failed shows **Retry setup**, which runs setup again in
the same Box. moor never retries a run on its own. The next due time is the
next try.

## Know when a run fires

- A new or re-enabled workflow fires only for due times after that moment.
- After downtime, moor fires the latest missed due time once.
- A run is skipped while the workflow's previous run is unfinished. A run
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

Three refused or timed-out runs in a row disable the workflow.
Runs you fired by hand do not count. `list_workflows` shows the reason. Ask to enable it again once you fix the
cause.

## Limits and the breaker

A run has one hour from the firing for its agent to start. A run that misses
that is refused, and its Box is closed. The run then has its timeout from the
agent's start. A run still working after that is stopped. Its Box stays open so
you can look inside.

When a Box's places die, moor does not resume the Box. Its run fails at its
step at once, with the reason `death_capped` gives. The run does not follow
the step's fail edge or try again, because either would wake the Box.

A missing secret or a full machine can refuse a run. The refusal is recorded
with its reason.

Three refused or timed-out runs in a row disable the workflow, and the
workflow shows why in place of its next run. Runs you fired by hand do not count, and one that
delivers resets the count. Enable it again once you fix the cause.

## Pause or remove a workflow

**Disable** stops the schedule. A run already in flight still finishes, and
tasks it already started stay. **Enable** turns it back on, for due times after
that moment. **Delete** removes it.

## From Claude Code

The same workflows can be made and run from Claude Code over
[MCP](mcp.md). Ask in plain words:

```text
Every weekday at 9:00 London time, start a Box on acme/web with acme/api beside
it to fix open Dependabot alerts and open a PR for each fix.
```

The agent calls `add_workflow` with these values.

- `name`, unique among your workflows. Adding the same name again replaces it.
- `cron`, five fields, and `timezone`, an IANA name such as `Europe/London`.
- `repos`, one or more `owner/name`, first one first. The controller's
  `box.also` never applies to a workflow.
- `config`, optional, a partial repo config in YAML that every Box starts
  with. See [Override for one Box](repo.md#override-for-one-box).
- `prompt`, `timeout_minutes` (180 by default), and `plan`, a `steps` list
  as the editor writes it.
- `expected_revision`, optional, the `revision` that `list_workflows` showed.
  The call is refused when the workflow changed after you read it.

Other tools:

- `run_workflow(name, expected_revision)` is Run now. The reply says
  `started` with the Box name, or `refused` with the reason.
- `answer_run(workflow, answer, note)` answers an approve step.
- `list_workflows` shows each workflow, its next due time, its plan and its
  latest run. `list_workflow_runs` lists each change oldest first, with an
  outcome and a `trigger` of `cron` or `manual`.
- `set_workflow_enabled` and `remove_workflow` pause and remove.

A cron run's Box is named `<workflow>/w<id>-<due time>`, and a run by hand
`<workflow>/w<id>-m<UTC time to the second>`.

## Move a workflow from `also`

Before workflows held a set of repos, a workflow named several `repos` and
started one Box for each. `also` named the repos every Box cloned beside its own.
Upgrading folds each stored workflow's `also` into its `repos`, after its one
repo, so it keeps running the same Box. A store that holds a workflow of several
repos refuses the upgrade, because each would start one Box where it started
many. Split such a workflow into one per repo, then upgrade.
