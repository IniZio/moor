# Workflow runs as plans

A workflow's prompt becomes a short plan of typed steps that a person reviews.
Each run freezes the plan and walks it. A step counts only when the agent
reports it with evidence, or when the controller's own check passes. moor
holds no job-specific logic. A dependency update is one prompt like any other.

The proof is four dependency-check workflows on the team cluster, one of them
on `IniZio/moor-pilot-example`.

## Why

Today a workflow is one prompt, and a run is finished once the agent has been
quiet for two minutes (`run_verdict` in
[`moor-core/src/decide/workflow/finished.rs`](https://github.com/IniZio/moor/blob/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor-core/src/decide/workflow/finished.rs)). Nothing says what done
means. A run that gave up and a run that delivered look the same, and the
owner opens the Box to find out.

## Scope

In:

- The plan. Its types, validation, storage, review and hand edits.
- The run. A frozen plan, a step pointer, an append-only history, and a phase
  kept apart from the outcome.
- The `report_step` tool, the code gate on evidence, the reminder rule, check
  steps, approve steps.
- The run view in the portal and the notification rule.

Out:

- Box tuning. The history leaves a slot for it (see
  [The tuning slot](#the-tuning-slot)). Its machine-agent side is designed
  elsewhere.
- A judge model call per work step. It comes after the core runs are stable.
- Plan tuning from run history.
- Any change to the driver seam in `moor-core`,
  [`moor/src/adapters/machine`](https://github.com/IniZio/moor/tree/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor/src/adapters/machine), `moor-machine-agent`,
  `moor-machine-runner` or the Proxy. This spec needs one Proxy route and
  names it as a dependency.

## Data shapes

### Plan

New module `workflow::plan` in `moor-core`. `Plan::parse` is the only
way to build a `Plan`, so every `Plan` in memory is valid.

```rust
/// A step's id: 1-32 bytes of `[a-z0-9-]`, starting with a letter. Stable across recompiles,
/// so history stays attached to the step it was written for.
pub struct StepId(String);

/// The SHA-256 of the prompt a plan was made from.
pub struct PromptHash([u8; 32]);

/// How often a step may be entered in one run: 1 to 5.
pub struct Tries(u8);

/// A file the agent leaves under `.moor/artifacts/`, as an artifact name
/// (`moor_core::artifact::valid_name`), or a prefix ending in `*` that one or more artifacts
/// must match, such as `before-*`. Artifacts are flat, so there are no directories.
pub struct EvidenceName(String);

/// One or more evidence names.
pub struct Evidence(Vec<EvidenceName>); // never empty

/// A validated plan. The first step is where a run starts.
pub struct Plan {
    steps: Vec<Step>,               // 1 to 20, ids unique, every target exists
    pub made_from: PromptHash,      // stale when it differs from the prompt's hash
    pub edited: bool,               // a person changed it after it was made
}

pub struct Step {
    pub id: StepId,
    pub title: String,              // at most 120 bytes
    pub max_tries: Tries,
    pub kind: StepKind,
}

pub enum StepKind {
    /// The agent works in the Box. The code gate decides whether the report counts.
    Work { instructions: String, done_when: String, evidence: Evidence, next: Next },
    /// The controller runs `command` in the Box. The exit code decides.
    Check { command: String, next: Next },
    /// The run waits for the workflow's owner to say yes or no.
    Approve { question: String, next: Next },
    /// The agent delivers: a pull request, a message, a file, whatever the prompt asks.
    /// Its report's note is the run's result. A deliver step ends the run.
    Deliver { instructions: String, done_when: String },
}

/// Where a run goes after a step.
pub struct Next {
    /// After a pass: report ok, gate passed, exit 0, or yes.
    pub ok: StepId,
    /// After a fail: report fail, exit non-zero, or no. `None` ends the run as Failed.
    pub fail: Option<StepId>,
}
```

What the types rule out:

- A deliver step with an onward edge. `Deliver` has no `Next`.
- An ok edge that ends the run without delivering. `ok` is a `StepId`, never
  "stop".
- A work step with no evidence. `Evidence` is never empty.
- A check step whose result the agent reports. A check has no report. The
  controller runs it.

`Plan::parse(text) -> Result<Plan, Vec<PlanError>>` returns every error at once
so a model or a person fixes them in one pass. It checks:

- 1 to 20 steps, unique ids, every `ok` and `fail` names a step.
- From every step some deliver step is reachable. No pocket can trap a run.
- Instructions, `done_when` and a command are at most 4 KiB each.

A plan needs no rule against loops. Each entry to a step counts as one try. A
step entered more often than its `max_tries` is exhausted and follows its
`fail` edge at once. If that target is exhausted too, the run ends as Failed.
So every run ends within `sum(max_tries)` entries.

`Plan::single(prompt)` is the plan of a workflow nobody planned. It is one
deliver step whose instructions are the prompt. Every workflow has a plan. There is
one finish path, not two.

The wire form is JSON with `serde`, and `schemars` derives its JSON Schema.
That schema is the "fixed schema" a model fills (see
[Where compile runs](#where-compile-runs)).

### Workflow

`Workflow` and `WorkflowDef` in [`moor-core/src/workflow.rs`](https://github.com/IniZio/moor/blob/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor-core/src/workflow.rs) gain:

```rust
pub struct Workflow {
    // ...every field it has today...
    pub plan: Plan,        // Plan::single(&prompt) when none was stored
    pub revision: u64,     // bumped on every put; plan edits check it
}

pub struct WorkflowDef {
    // ...every field it has today...
    pub plan: Option<Plan>, // None stores no plan, and reads back as Plan::single
}
```

Cron, timezone, repos, `also`, config, timeout, origin, enable state, the
and the overlap skip are unchanged.

### Run

`WorkflowOutcome` mixes progress (`Starting`, `Started`) with endings. It is
replaced by `RunState`, which keeps the phase apart from the outcome. All
callers move in the same PR and `WorkflowOutcome` is deleted.

```rust
/// The database id of a run (`workflow_runs.id`, which exists today).
pub struct RunId(i64);

pub struct WorkflowRun {
    pub id: RunId,
    pub workflow: WorkflowId,
    pub repo: ForgeRepo,
    pub due_at: Ts,
    pub state: RunState,
    pub recorded_at: Ts,
}

pub enum RunState {
    /// The firing is claimed and its Box is being started. Was `Starting`.
    Queued,
    /// The Box is up and the run is at step `at`, try `try_n`. Was `Started`.
    Running { box_id: BoxId, at: StepId, try_n: u8 },
    /// An approve step waits for the owner. The Box may sleep.
    Waiting { box_id: BoxId, at: StepId, try_n: u8, since: Ts },
    /// Nothing more will happen.
    Done(RunEnd),
}

pub enum RunEnd {
    /// The previous run's Box was still open, so none was started.
    SkippedOverlap,
    /// The start was refused, or no agent could run in the Box.
    Refused { detail: String },
    /// The run had a Box and ended.
    Ended { box_id: BoxId, ended_at: Ts, outcome: RunOutcome },
}

/// How a run that had a Box ended. Only moor's own facts. What a delivery means
/// ("same", "worse, draft PR") is the agent's text in `result`.
pub enum RunOutcome {
    /// A deliver step reported ok.
    Delivered { result: String },
    /// A step failed with no `fail` edge, or its fail target was exhausted too.
    Failed { at: StepId, reason: String },
    /// The agent stopped without reporting step `at`, after one reminder.
    Incomplete { at: StepId },
    /// The owner said no at an approve step with no `fail` edge.
    Declined { at: StepId },
    /// The run worked past its timeout.
    TimedOut { at: StepId },
}

/// The phase as shown in lists, the way GitHub Actions shows a run's status.
pub enum RunPhase { Queued, Running, Waiting, Done }

impl RunState {
    pub const fn phase(&self) -> RunPhase { /* one arm per variant */ }
    pub const fn box_id(&self) -> Option<BoxId> { /* as WorkflowOutcome::box_id */ }
}
```

What the types rule out:

- A step pointer on a run that never got a Box. Only `Running` and `Waiting`
  hold `at`.
- An outcome on a run still in flight. The outcome lives inside `Done`.
- A skipped run with a Box. `SkippedOverlap` holds none.

`RunEnd` in the workflow module today (`Finished`, `TimedOut`) is the argument of
`end_workflow_run`. It is folded into `RunOutcome` and removed.

### Reports and history

```rust
pub enum StepResult { Ok, Fail }

/// What the agent sends with `report_step`. The run and the Box come from the
/// transport, never from an argument.
pub struct StepReport {
    pub step: StepId,
    pub result: StepResult,
    pub evidence: Vec<String>,   // artifact names it left; checked against the step's Evidence
    pub note: String,            // at most 2 KiB; a deliver step's note is the run's result
}

pub enum Answer { Yes, No }

/// One line of a run's history. Rows are only appended.
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryEntry {
    /// The run reached a step; `try_n` counts from 1.
    Entered { step: StepId, try_n: u8 },
    /// The step's instructions reached the agent, in a report reply or a steer.
    Sent { step: StepId, try_n: u8 },
    /// The agent reported, and the code gate passed.
    Reported { step: StepId, try_n: u8, report: StepReport },
    /// The agent reported, and the code gate refused it.
    GateRefused { step: StepId, try_n: u8, missing: Vec<String> },
    /// The controller ran a check.
    Checked { step: StepId, try_n: u8, exit: i32, tail: String }, // tail: last 4 KiB
    /// The agent went quiet without a report and was reminded once.
    Reminded { step: StepId, try_n: u8 },
    /// The owner answered an approve step.
    Answered { step: StepId, by: UserId, answer: Answer, note: Option<String> },
    /// The step was entered more than its max_tries and followed its fail edge.
    Exhausted { step: StepId },
    /// The run ended.
    Ended { phase_before: RunPhase, outcome: RunOutcome },
}
```

Tries come from history. A step's try is the count of its `Entered` lines. The
`try_n` on `RunState` is a copy written in the same transaction, used for the
compare-and-set below. "One reminder" holds across restarts and replicas
because the `Reminded` line is in the store, not in memory.

### The tuning slot

History is tagged JSON. The tuning unit adds variants such as
`Tuned { kind, what, why, status }` and its own table for proposals to save
into repo config. Nothing in this spec reads them, and the run view renders an
unknown kind as a plain line.

### Events

`WorkflowEvent` in [`moor-core/src/domain/event/kind/workflow.rs`](https://github.com/IniZio/moor/blob/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor-core/src/domain/event/kind/workflow.rs) gains
four Box events. `list_events` and a Box Monitor then show run progress on the
Box itself. A webhook already picks the Box event kinds it receives, and
[`moor/src/store/apply.rs`](https://github.com/IniZio/moor/blob/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor/src/store/apply.rs#L378) queues a delivery when an event is recorded.

```rust
StepEntered { workflow: String, step: String, try_n: u8 },
StepReported { workflow: String, step: String, result: String },
RunWaiting { workflow: String, step: String, question: String },
RunFailed { workflow: String, outcome: String, step: String, reason: String },
```

`RunFailed` is recorded for `Failed`, `Incomplete`, `TimedOut` and `Refused`.
A delivered run records no run event.

## Store

One migration, `V85__workflow_plans`, carries every column the stack needs, so
later PRs touch no schema.

`workflows`:

| Column | Type | Meaning |
| --- | --- | --- |
| `plan` | text, null | The plan as JSON. Null reads as `Plan::single(prompt)`. |
| `revision` | int, not null, default 0 | Bumped on every put. |

`workflow_runs`:

| Column | Type | Meaning |
| --- | --- | --- |
| `plan` | text, null | The frozen plan, copied from the workflow at claim time. Null for runs before V85; they read as `Plan::single`. |
| `step` | text, null | The pointer. Set while `running` or `waiting`. |
| `try_n` | int, null | The pointer's try. Set with `step`. |
| `waiting_since` | int, null | Set while `waiting`. |
| `ended_step` | text, null | The step a failed, incomplete, declined or timed-out run ended at. |

`detail` keeps the refusal reason and also holds a failed run's reason and a
delivered run's result.

The `outcome` column's check grows from six values to ten:

| Stored | Reads as | Note |
| --- | --- | --- |
| `starting` | `Queued` | unchanged |
| `started` | `Running` | unchanged |
| `waiting` | `Waiting` | new |
| `finished` | `Ended(Delivered)` | unchanged tag; old rows read with an empty result |
| `failed` | `Ended(Failed)` | new |
| `incomplete` | `Ended(Incomplete)` | new |
| `declined` | `Ended(Declined)` | new |
| `timed_out` | `Ended(TimedOut)` | unchanged; old rows read `at` as the single step |
| `skipped_overlap` | `Done(SkippedOverlap)` | unchanged |
| `refused` | `Done(Refused)` | unchanged |

No backfill is needed. Every old tag keeps its meaning, so queries that filter
on `starting`, `started`, `skipped_overlap` and `refused` stay correct.
`open_workflow_runs` adds `waiting` to its `IN` list.

Postgres replaces the check in place. SQLite cannot, so it rebuilds
`workflow_runs` the way `V80__shared_machines` rebuilds `user_machines_v2`.
No table has a foreign key to `workflow_runs` today, so the rebuild is copy,
drop and rename. It recreates the unique key on `(workflow, repo, due_at)` and
the `workflow_runs_workflow` index. The rebuild runs first, and the history
table with its foreign key is created after it.

New table `workflow_run_history`:

| Column | Type |
| --- | --- |
| `run` | int, not null, FK `workflow_runs.id` on delete cascade |
| `n` | int, not null; unique with `run` |
| `at` | int, not null |
| `entry` | text, not null; a `HistoryEntry` as JSON |

New index `workflow_runs_box` on `workflow_runs(box)`, since `report_step`
finds a run by its Box.

Every move of a run is one transaction in `Store::numbered`. It compares and
sets the row on `(id, outcome, step, try_n)` as last read. It appends the
history line at `n = max + 1` and takes the change number. A report or a
tick that read a stale row changes nothing and reports that, so a retried
report and a double tick both converge.

### Act, then record

Some moves act outside the store. They steer the agent or run a check in the
Box. Each act comes first, and its history line is written after it. A crash
between the two leaves no line, so the next tick acts again. Every act is
therefore safe to repeat.

| Act | Line written after it | A crash between leaves |
| --- | --- | --- |
| Steer the next step's instructions | `Sent { step, try_n }` | The agent may get the same instructions twice. |
| Steer the reminder | `Reminded` | The agent may get the reminder twice. It still counts as one, because only the line counts. |
| Run a check | `Checked` | The check runs again. Checks must be safe to rerun, as tests and builds are. |
| Steer the answer to an approve step | `Sent` for the step after it | The agent may get the answer twice. |

`report_step` is the exception. Its reply carries the next instructions, so the
pointer move and the `Sent` line share one transaction. If the reply is lost,
the agent reports again and gets "already recorded" with the same
instructions.

## Where compile runs

The author's own agent compiles. `add_workflow` gains an optional `plan`
argument whose JSON Schema is derived from the `Plan` wire type. The model
that calls `add_workflow` already turns plain words into `cron`, `repos` and
`prompt`. It fills `plan` in the same call. moor validates with `Plan::parse`
and refuses the call with every error listed. The model fixes them and calls
again.

The three options, by new code:

| Option | New code | Cost |
| --- | --- | --- |
| The calling agent fills `plan` | A tool argument, a tool description, `Plan::parse` | The portal has no compiler. |
| The controller calls a model | A model client, a key in the controller's config, retries, a schema prompt, a timeout | The controller holds no model client or key today. All of it is new, and it ties moor to one vendor. |
| An agent in a Box | A compile Box per save, a result channel | A Box takes minutes to start. Review waits on it. |

The first option needs no model plumbing in moor, and it works with any agent
that speaks MCP. The `add_workflow` description tells the model how to make a
plan:

- One step per finish line.
- A check step for anything a command can decide.
- An approve step before anything risky the prompt names.
- `max_tries` on a fix step that loops back.
- Keep the ids of an existing plan's unchanged steps. The model reads that
  plan with `list_workflows`.
- Read `get_repo_config`. Say in the reply when a step needs a host or a
  credential that the repo does not grant. moor does not judge this itself,
  because that would be job-specific logic.

### Review

- **In chat.** `add_workflow` saves nothing until the plan parses. Its reply
  renders the stored plan as a numbered list next to the prompt. The model
  shows it to the author, and the author changes it in plain words or edits
  the JSON. Each change is another `add_workflow` with `expected_revision`, as
  in `set_repo_config`.
- **In the portal.** A new **Workflows** page beside **Repos**
  ([`moor-gate/src/login/repos.rs`](https://github.com/IniZio/moor/blob/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor-gate/src/login/repos.rs) is the pattern). It shows the prompt
  and the plan side by side, the plan as editable JSON, and saves with the
  revision the page read. A 409 reload works as it does for repo config. A
  plan saved here is marked `edited`.
- **Stale.** `list_workflows` and the page show `plan_stale` when the plan's
  `made_from` differs from the prompt's hash. A stale plan still runs. The
  prompt is the stated intent, and the plan is what runs.

A run freezes the plan at claim time into `workflow_runs.plan`. An edit never
changes a run in flight.

## Reporting a step

### The tool

The agent in the Box calls `report_step` on a new MCP server named `moor`.
Every Box gets it through `UserConfig::shared_mcp`, as it gets the machine
service's server.

```text
report_step(step, result: "ok" | "fail", evidence: [name], note)
```

Its reply always says what to do next, so the agent never guesses.

| Case | Reply | Run |
| --- | --- | --- |
| The Box has no running run | An error saying so. | unchanged |
| `step` is not the current step | The current step and its instructions. | unchanged |
| The gate refuses | What is missing, too small or too large. | `GateRefused` appended. A second refusal in the same try fails the step. |
| Passed, next is work or deliver | The next step's title, instructions, done-when and evidence. | Pointer moves. |
| Passed, next is a check | "End your turn. moor runs the check and sends you the result." | Pointer moves. |
| Passed, next is approve | "End your turn. moor asks the owner and sends you the answer." | `Waiting` |
| Passed, at a deliver step | "Done." | `Ended(Delivered)` with the note as the result. |
| The same report again | "Already recorded" and the next step again. | unchanged |

### The code gate

A work step's report passes when every `EvidenceName` of the step matches a
top-level regular file in the Box's artifacts folder. Each file must not be
empty and must be at most 25 MiB. A report with `result: fail` passes the
gate when its note is not empty. A step that cannot be done says why, and its
`fail` edge decides what happens. A deliver step has no evidence list, so its
report passes when its note, the run's result, is not empty.

The gate runs while the agent waits on the call. In the Box, it runs the index
script that
[`moor/src/ops/common/artifacts.rs`](https://github.com/IniZio/moor/blob/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor/src/ops/common/artifacts.rs)
already uses, which lists the name, size and sha256 of each top-level file. It
does not wait for the turn-end copy into the store. That copy runs later and
makes the same files downloadable from the run view. An exec may run while the
agent works, as the exec op's own doc says
([`moor/src/ops/exec.rs`](https://github.com/IniZio/moor/blob/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor/src/ops/exec.rs#L3)).

### Transport and identity

The Box's identity must come from the transport. The gate today accepts only
a person's OIDC token ([`moor-gate/src/mcp.rs`](https://github.com/IniZio/moor/blob/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor-gate/src/mcp.rs)), and a Box holds none.

The machine service already solves this. The shared Proxy routes
`machine.moor.internal` to the runner and stamps `X-Moor-*` headers that name
the Box, and it drops any the Box sent itself
([`moor/src/proxy/front/machine.rs`](https://github.com/IniZio/moor/blob/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor/src/proxy/front/machine.rs)). `report_step` uses the same shape.

- The Proxy routes `moor.moor.internal` to a Box-facing listener of moor-gate
  and stamps `X-Moor-Box: <id>`. This is the one Proxy change. The owner of
  the Proxy code makes it.
- That listener serves only the `moor` server's tools and trusts
  `X-Moor-Box` because only the Proxy reaches it. It forwards a
  `StepReportReq { box_id, report }` to the controller.
- No token sits in the Box.

Until the route exists, tests call the listener with the header set, as
[`moor-machine-runner/tests`](https://github.com/IniZio/moor/tree/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor-machine-runner/tests) does.

### How the controller learns where a run stands

Today `run_verdict` answers Running, Finished (quiet for 120 s, agent ended,
Box gone) or Broken. Finished stops meaning "the run is done". It is renamed
`Quiet`, and the tick decides from the plan.

| Verdict | Pointer at work or deliver | Pointer at check | Waiting |
| --- | --- | --- | --- |
| Running | nothing | nothing | nothing |
| Quiet, no `Reminded` this try | Steer the reminder. Append `Reminded`. | Run the check. | nothing. The Box may sleep. |
| Quiet for 120 s after `Reminded` | `Ended(Incomplete)` | (n/a) | nothing |
| Agent ended, no `Reminded` this try | Steer the reminder. The steer queue relaunches the agent on its stored session. Append `Reminded`. | Run the check. | nothing |
| Agent ended after `Reminded` | `Ended(Incomplete)` | (n/a) | nothing |
| Box gone or closed | `Ended(Incomplete)`; nobody is left to remind | `Ended(Incomplete)` | `Ended(Incomplete)` |
| Broken | `Refused`, as today | `Refused` | `Refused` |

Once a step has been reported, the reminder counter starts over. A run is
Incomplete only when one step goes quiet twice without a report.

The reminder is one steer, the same for every workflow:

> Step `<id>` ("<title>") has no report yet. Call `report_step` with result
> ok and the evidence it names (`<names>`). Or call it with result fail and a
> note that says why the step cannot be done.

The Box is kept open for inspection, as a timed-out run's Box is today.

### Check steps

The controller runs a check when the pointer is at it and the agent is quiet,
so nothing edits the work tree under the check. It uses the existing exec op
([`moor/src/ops/exec.rs`](https://github.com/IniZio/moor/blob/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor/src/ops/exec.rs)) in the Box's work place. The deadline is the
smaller of the run's remaining timeout and 30 minutes. Exit 0 follows `ok`.
Any other exit code, or the deadline passing, follows `fail`. A lost stream is
not a result. A machine agent restart cuts open exec streams, and the command
goes on in the work place. The controller appends `CheckLost` and runs the
check again on the next tick, so a check command must be safe to repeat. Two
lost streams in one try follow `fail`. The last 4 KiB of output
goes into the `Checked` line. Then the controller steers the agent with the
result and the next step's instructions, unless the next step is another
check.

### Approve steps

Entering an approve step sets `Waiting`. The Box is left to autosleep as any
quiet Box is. The owner answers with a new user tool, `answer_run(workflow,
repo, answer, note)`, on the gate under `moor:write`, or with Yes and No
buttons on the portal's run view. Only the workflow's owner may answer.

- Yes moves to `ok`.
- No moves to `fail`. When the step has no `fail` edge, the run ends
  `Declined`.
- The next step's instructions go out as a steer. A steer queued to an asleep
  Box wakes it ([`moor/src/ops/steer/queue.rs`](https://github.com/IniZio/moor/blob/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor/src/ops/steer/queue.rs) calls
  `wake_and_revive`).

### Phase against the existing rules

| State | Overlap `blocks` | Expiry | Start limit |
| --- | --- | --- | --- |
| `Queued` | yes | no | yes |
| `Running` | yes | yes | yes, until the agent starts |
| `Waiting` | yes | the clock stops while waiting | no |
| `Delivered` | no | (n/a) | (n/a) |
| `Failed`, `Incomplete`, `TimedOut`, `Refused` | `TimedOut` only while its Box is open, as today | (n/a) | (n/a) |
| `Declined` | no | (n/a) | (n/a) |
| `SkippedOverlap` | no | (n/a) | (n/a) |

No ending disables the workflow. A failed run shows as failed and the next due time fires.

Expiry counts working time. That is the wall clock since the first launch,
minus each span between a `Waiting` move and its `Answered` line in history.
`workflow_expired` takes those spans as one more fact.

`PriorRun` gains no variant. `Waiting` maps to `PriorRun::Started`, and the
ended outcomes map to `Finished` or `TimedOut` as their row says.

## What people see

- **`list_workflow_runs` and `list_workflows`.** `LastRunView.outcome` keeps
  the stored tag, now with the four new values. `LastRunView` gains `phase`
  (`queued`, `running`, `waiting`, `done`), `step` (the pointer, or the step a
  run ended at) and `result` (a delivered run's note). A front that reads only
  `outcome` keeps working.
- **Runs list.** The portal's Workflows page lists runs newest first. Each
  line has a status icon, repo, trigger, duration, and outcome or phase.
- **Run view.** It shows the frozen plan as a list of steps. Each step
  expands to its history lines, its check output and links to its evidence
  artifacts. A waiting run shows the question with Yes and No.
- **Notifications.** The owner adds a webhook for the `run_waiting` and
  `run_failed` event kinds, and moor-notify posts each one from the existing
  outbox. A delivered run records no run event, so it is silent. Anything more
  is the prompt's job.

## The Box's prompt

moor wraps the workflow prompt with one generic preamble. The preamble is the
same for every workflow:

```text
<workflow prompt>

moor runs this as a plan of <n> steps. Do only the current step, then call
report_step. Leave each evidence file in .moor/artifacts/.

Current step: <id>, "<title>"
<instructions>
Done when: <done_when>
Evidence: <names>
```

Each later step arrives in the reply of `report_step` or as a steer.

## PR stack

Each PR ends in a check someone can rerun. The first PR is useful alone. Unit
test paths are where the existing tests live.

A live check needs a firing now. The private controller of verify-moor-live
sets `workflow_min_interval_secs: 60` in its controller config, and the test
workflow uses the cron `* * * * *`. It fires within a minute.

The PR that needs the Proxy route comes after every PR that does not.

1. **`feat(workflow): runs show a phase, an outcome and a history`**
   - V85 in full.
   - `RunState` replaces `WorkflowOutcome`, and every caller moves in this PR.
   - History lines for queued, running and ended.
   - `Workflow.plan` is always `Plan::single`, so behavior is unchanged. Quiet
     still ends a run, now recorded as `Delivered`.
   - `list_workflow_runs` and `list_workflows` show phase and outcome.
   - The owner sees it at once. An existing workflow's runs read "done,
     delivered" or "running", with a history.
   - Check. Run `cargo test -p moor-schema` (V85 on SQLite and Postgres in
     [`moor-schema/src/tests`](https://github.com/IniZio/moor/tree/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor-schema/src/tests)), `cargo test -p moor-core
     decide::workflow`, `cargo test -p moor ops::workflow`, then
     run verify-moor-live with a workflow fired on `IniZio/moor-pilot-example` and
     its `list_workflow_runs` output saved.
2. **`feat(workflow): a workflow carries a reviewed plan`**
   - The `Plan` types and `Plan::parse`.
   - `add_workflow` takes `plan` and `expected_revision`.
   - `list_workflows` shows the plan and `plan_stale`.
   - Runs freeze the plan and get the preamble.
   - Work steps still advance on quiet. The tick steers the next step.
   - Check. Parse tests in [`moor-core/src/workflow/tests.rs`](https://github.com/IniZio/moor/blob/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor-core/src/workflow/tests.rs) (every
     rule, every error returned at once). An ops test where a two-step plan
     advances on quiet. Live. A two-step plan on the pilot repo, with both
     `Entered` lines in history.
3. **`feat(workflow): moor runs check steps`**
   - A pure `decide::workflow::advance(plan, history, event) -> Move`.
   - Check steps run through the exec op, with `max_tries` and exhaustion.
   - Check. Decide tests in [`moor-core/src/decide/workflow/tests.rs`](https://github.com/IniZio/moor/blob/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor-core/src/decide/workflow/tests.rs) for
     an ok path, a fail path, a loop that exhausts, and a double exhaustion
     that ends Failed. An ops test with a fake exec returning 0 and 1. Live. A
     plan whose check is `test -f package.json`.
4. **`feat(workflow): approve steps pause a run`**
   - `Waiting`, `answer_run`, and expiry that stops while waiting.
   - Check. Decide tests for the phase table above. Live. A plan with an
     approve step. The run waits and the Box sleeps. `answer_run` yes wakes
     it and the run goes on.
5. **`feat(gate): workflows and runs in the portal`**
   - The Workflows page, the plan editor with revision, the runs list, the
     run view, and Yes and No.
   - Check. Tests in
     [`moor-gate/src/login/tests.rs`](https://github.com/IniZio/moor/blob/8d10b84d2f70ea0e8f9473243e68c5cb5c096b2f/crates/moor-gate/src/login/tests.rs)
     for the 409 path. Then take agent-browser screenshots of a delivered run
     and a waiting run on the live controller.
6. **`feat(gate): agents report steps with evidence`**
   - The `moor` Box MCP server and the Box-facing gate listener.
   - `report_step`, the code gate, and the reminder rule from the table.
   - Quiet no longer ends a work step.
   - Needs the Proxy route for `moor.moor.internal`, which the owner of the
     Proxy code adds.
   - Check. Gate tests that set `X-Moor-Box` directly. Ops tests for the
     reminder across a controller restart (one `Reminded`, then
     `Incomplete`). Live. One run whose agent reports every step, and one
     whose prompt says never to report, which ends Incomplete after one
     reminder.
7. **`feat(workflow): failed and waiting runs notify their owner`**
   - The `run_waiting` and `run_failed` Box events.
   - Check. Webhook outbox tests. Live. A local receiver subscribed to both
     kinds gets exactly one delivery for an Incomplete run and none for a
     Delivered one.
8. **Team cluster.** Deploy is the operator's call.
   - Four dependency-check workflows, each with a plan reviewed in chat, on
     four repos including `IniZio/moor-pilot-example`.
   - Check. Each run reaches `Done` with an outcome, without opening its Box.
     One row per run goes in the grading table.

## Open questions

1. **The Proxy route.** `report_step` needs `moor.moor.internal` routed to the
   gate with `X-Moor-Box` stamped. Recommendation. The owner of the Proxy code
   adds it in the machine-service shape. If it slips, PRs 1-5 still land. PR 6
   waits, because a per-run bearer token in the Box's environment would break
   the rule that a Box holds no tokens.
2. **Compile in the portal.** A portal-only author gets no model. Recommendation.
   Accept for the pilot. The portal creates `Plan::single` from the prompt and
   allows a hand-written plan. Add a controller model call only if people who
   never use an agent start writing workflows.
3. **"Hold" as an answer.** The design page lists yes, no and hold.
   Recommendation. Drop hold. Not answering already holds, and the clock
   stops while waiting.
4. **A wait with no answer.** Recommendation. After 7 days, end the run
   `Declined` with the reason "no answer", so the next firing is not skipped
   forever.
5. **Evidence in folders.** Artifacts are flat. Recommendation. Use prefix names
   (`before-*`) in place of `before/`. The compiler is told so.
6. **Box rebuilt mid-run.** Under the Box hands model a rebuild keeps the Box
   id. Recommendation. The run keeps its pointer and the steer queue
   re-sends the current step after the rebuild. The machine-agent side needs
   nothing from this spec.
7. **No `when` on a step.** The design page gives a step a condition, such as
   "only if a major bump is needed". This spec drops it. A work step that
   reports `fail` takes its `fail` edge, and that edge can lead to the approve
   step. Recommendation. Keep one branching rule. Add `when` only if the
   compiler cannot express a plan with edges alone.
8. **Capability warnings.** The design page has moor flag a step that needs a
   host or a credential the repo does not grant. This spec asks the compiling
   model to do it, from `get_repo_config`. Recommendation. Keep it out of moor
   until the egress work gives moor a typed list of what a repo grants. Then
   moor can check each step's declared needs against it without reading step
   text.
