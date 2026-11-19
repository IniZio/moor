# End-user experience

moor's web portal becomes the main place to work. A person starts a task there, watches it live, steps in through a terminal in the browser, and reviews the result. Claude Code over MCP stays as a second way in. This spec sets the target and the order to build it. It replaces the earlier version of this page and the onboarding spec.

## Decisions this spec rests on

The owner decided these on 2026-11-18.

| # | Decision |
| --- | --- |
| 7 | The web portal is the main place to work. Claude Code over MCP stays as a second way in. The herdr front and other redundant parts go. |
| 8 | Design for developers on the team first. QA, PMs and designers must be able to start a task and review its evidence without a terminal. |
| 9 | Stepping in happens in the browser, as a live terminal on the Box's real Claude session. "Open in VS Code over SSH" comes later as a plug-in. |

The earlier version of this page said the account page never starts a Box. That rule is gone.

### Why the last web front failed

moor-web v6 to v8 were rejected on 2026-10-27. They were built before an interview, a spec and a side-by-side study of bb and thinkrail. This spec does those three first. Nothing is built until the owner has read it.

## Who

- **Developer.** Hands work to a Box, checks on it, takes over when needed, and merges the PR. Uses the web and, at times, Claude Code.
- **QA, PM or designer.** Starts a task in plain words, answers the agent's questions, and reviews screenshots and the PR. Never opens the terminal.
- **Operator.** Runs moor for the team. Also a developer on it.

## Surfaces

| Surface | Job |
| --- | --- |
| Web portal | Everything a person does: start, watch, answer, steer, step in, review, schedule, set up repos and secrets. |
| Claude Code over MCP | Start, check and steer Boxes from a person's own agent. Same verbs, same states. |
| Webhooks | Tell another system that a Box needs someone. |

Dropped surfaces:

- **The herdr front**, its picker, panel and Box cards. The browser terminal replaces it.
- **The moor-hc bridge for teammates.** It existed to reach a Box's session from a terminal. It goes once the browser terminal ships.
- **The moor plugin's local event listener.** Events reach Claude Code over the HTTP MCP connection instead.

## References

Two research passes on 2026-11-18 read these products. The hosted products were studied from their docs only, with no screenshots. bb, thinkrail and agentrun were studied from their source and screenshots.

| Product | Strongest idea for moor |
| --- | --- |
| thinkrail | One attention dot drives the list, the switcher and notifications. Each turn ends with a receipt line and a Review button. The PR is a state machine in the top bar. |
| bb | A composer footer names the target (repo, machine, branch) before you send. Child work is nested under its parent. |
| agentrun | An "Input required" banner quotes the question. Take over and hand back the agent's browser. Schedules in words with Run now. |
| Devin | One progress timeline, where each step links to the terminal or browser it used. Read-only side questions during a run. |
| Copilot coding agent | Steer applies after the current tool call. Stop keeps pushed work. The requester cannot approve their own agent PR. |
| Jules | A plan gate before code, revised in chat. Avoid its auto-approve on a timer. |
| Cursor | Screenshots, videos and logs attached to the PR. |
| Linear | A derived state set with an explicit awaiting-input state. A human owner stays on every agent task. Recurring issues as the model for schedules. |
| Codemod | Checks shown inline on each task's card across many repos. |
| crabbox | The list defaults to active work. |
| Outpost | The in-house prototype. Live terminal and steering worked. It had one status dot, no review surface and no durable notifications. |

What not to copy:

- A composer-first home with model pickers. Most visits are to check on work, not to start it.
- IDE chrome, such as editors, draggable layouts and plugin-placed panes.
- A terminal as the only live view. A PM can't read one.
- A cost and token dashboard as the home page.
- Toasts with no durable inbox.
- Approval by timeout.

## The words

A person sees four nouns. **Task** is what they asked for. **Box** is the computer a task runs in. **Workflow** is a task on a schedule. **Run** is one firing of a workflow. The portal says "task" wherever a person starts or reviews work, and "Box" only where the computer matters. That covers the terminal, size, machine and sleep.
## State

Every surface shows the same states. The controller derives them from Box events, so no front computes its own.

```
queued → setting up → working ⇄ needs you
                         ↓
                       done → asleep → closed
          failed (setup failed, agent crashed, refused)
```

- **needs you** is one field, set when the agent asks a question, wants a permission, an approve step waits, or setup needs a person. It drives the list badge, the switcher order, the inbox and notifications. It is the most important state on the page.
- **done** means the agent finished its turn and is quiet. It is not "closed". The Box sleeps on its own later.
- **stale** marks a working Box with no event for 30 minutes. It shows as a warning on the row, not as its own state.

## Screens

### Home

The home is a list of tasks, with the ones that need you first. It is not a dashboard.

```
moor   [⌘K  Find a task]                                [+ New task]   inbox 2   you
──────────────────────────────────────────────────────────────────────────────────
NEEDS YOU · 2
 ●  acme/web   Fix login typo        "Allow npm run e2e?"           2m    you
 ●  acme/api   Deps check  (run 14)  Approve: open PR for 3 bumps?  1h    you
WORKING · 3
 ◐  acme/web   Add dark mode         editing src/theme.ts           14m   ana
 ◐  acme/api   Flaky test hunt       running pytest -k auth         3m    you
 ◌  acme/ios   Snapshot update       setting up · 2 of 5 lines      1m    you
DONE · 4                                                     PR  checks
 ✓  acme/web   Login copy            PR #212 open                  ✓ 3/3   1h
 ...
Settled · 18  ▸
```

- Each row shows state, repo, title, the one line that matters now, age and owner.
- "Settled" holds tasks whose PR merged or closed, tasks that were closed, and done tasks idle for 7 days. It is folded by default, as in thinkrail. A row's Keep active option holds it out of Settled.
- Filters cover mine or everyone's, repo and workflow. Mine is the default.
- ⌘K opens a switcher with needs-you tasks first.

### New task

```
┌─────────────────────────────────────────────────────────────────┐
│ What should the agent do?                                       │
│                                                                 │
│ Fix the typo on the login page and open a PR with a screenshot. │
│                                                                 │
├─────────────────────────────────────────────────────────────────┤
│ acme/web + acme/api ▾   from main ▾   on engine-03 ▾   [ ] Plan first   [Start] │
└─────────────────────────────────────────────────────────────────┘
```

- The footer names the target before you send, as bb does. The repo defaults to the last one used.
- "Plan first" makes the agent propose a plan and wait for approval before it changes anything. It reuses the workflow plan shape. Off by default.
- The repo picker takes one or more repos. The first is where the agent starts, and the rest are cloned beside it. See [Repos of a task](#repos-of-a-task).
- A QA or PM user needs nothing else. Advanced fields (Box size, timeout) sit behind "More".

### Task

```
acme/web ▸ Fix login typo ▸ moor/42-fix-login-typo · engine-03     ● needs you
PR: none yet                                                   [Stop] [⋯]
──────────────────────────────────────────────────────────────────────────────
 Timeline | Terminal | Changes +3 −1 | Evidence 2                Plan
                                                                 ✓ review
 ┌ Your input is needed ──────────────────────────────────────┐ ● fix
 │ Allow npm run e2e?                                         │ ○ test
 │ [Deny] [Allow once] [Allow for this task]                  │ ○ deliver
 └────────────────────────────────────────────────────────────┘
 You 10:02   Fix the typo on the login page and open a PR…    Running
 Agent       Read 2 files, searched 1 time                ▸   setup ok
 Agent       Edited src/login.tsx                         ▸   e2e (bg) 3m
 Agent       "The typo was in the label. Running e2e next."
 ── turn 1 · 4 tool calls · 1 file changed +3 −1 · Review · 1m 55s ──
──────────────────────────────────────────────────────────────────────────────
 (Steer|Interrupt)  Steer the agent…                                [Send]
```

- **The header** is a breadcrumb of repo, task, branch and machine, then the state. Below it sits the PR as a state machine. It moves through none yet, draft, open with checks, approved and merged. The button always names the next step.
- **Timeline** is the default tab. It folds routine tool calls into one line and never folds errors. Each turn ends with a receipt that moor computes from git, not from the agent's words.
- **Terminal** is the Box's real Claude session, live, with keyboard. It uses xterm.js with the WebGL renderer, pinned to one version. ghostty-web was tried first and dropped, because it cannot report the mouse to Claude. Claude stays full-screen, so the wheel scrolls Claude's own view back to its first line. It is where a developer takes over. It closes nothing when the tab closes.
- **Changes** is the diff against the base branch, file by file.
- **Evidence** is the artifact gallery. It holds screenshots, videos, logs and step reports.
- **The right rail** shows the plan's steps when there is one, and the background jobs.
- **The waiting card** quotes the question. An answer covers once, or this task. "Always" belongs in repo settings, not on the card. Sending a steer while the card is open denies the request and passes the message on, and the card says so.

Only Stop sits in the header. Close is under ⋯ with a confirm. Steer and Interrupt both need a message, so they are two modes of the message box at the bottom.

Steering verbs:

| Verb | What happens | What survives |
| --- | --- | --- |
| Steer | The message reaches the agent after its current tool call. | Everything. |
| Interrupt | The agent stops its turn now and reads the message. | Everything. |
| Stop | The agent ends and the Box sleeps. A steer wakes it. | Files, branch, session. |
| Close | The Box is removed. | Pushed commits and evidence. |

### Workflows

- A list of workflows with the schedule in words ("Weekdays at 9:00 London"), its repos, the next run and the last result.
- "Make this recurring" on a finished task starts a workflow with its prompt, as Devin and Linear do.
- The plan is shown as numbered steps, editable in place. JSON stays behind "Edit as JSON".
- **Run now**, to test before the schedule fires. When the last run failed, Run now sits beside it.
- A run opens the same Task page, with the plan rail and the step history.
- A workflow the breaker paused says why, in one line, at the top.

### Repos of a task

A task or a workflow holds an ordered set of repos, all cloned into one Box.

- The first repo is the starting point. The agent works there, and its branch and PR go there.
- Each other repo is cloned beside it, as `also` does today. One the agent changed gets its own branch and PR.
- A workflow run is one Box with all its repos. A workflow no longer starts one Box per repo.
- To run the same schedule on separate projects, make one workflow per project. "Duplicate" on a workflow makes that one click.
- The portal shows the set as one picker, first repo first. The words "also" and "beside" leave the portal.

Why. Each pilot workflow names one repo, and one of them clones six more with `also`. Nobody used one Box per repo. People asked for a workflow over several repos and could not find how.

What changes in moor.

- `add_workflow` and `start_box` take `repos`, an ordered list. `also` goes.
- A run has no repo of its own. Its repo is the first of the set.
- The breaker, `last_due` and the open-run check apply per workflow, not per repo.
- `run_workflow` loses its `repo` argument.
- A Box's name comes from the workflow and the trigger, not the repo.
- A migration folds each stored workflow's `also` into `repos`, after its one repo. A stored workflow with more than one repo is refused. None exists today.
- These land after the pilot's proof runs end, so no migration runs under an open run.

### Inbox

A durable list of everything that needed you, newest first. It survives a reload. Unread items light the inbox count. Browser notifications and webhooks are fed from the same items. needs_input, run_waiting and run_failed are on by default.

### Repos and account

Repos hold the shared repo config, edited as today, with each field marked as applied now or from the next Box. Account holds secrets, machines and connect lines. Both stay much as they are and move into a Settings area, out of the main navigation.

## What the screens need from moor

A research pass on 2026-11-18 read origin/develop. Most screens need data moor already has. Four things are missing.

| Screen part | Data today | Gap |
| --- | --- | --- |
| Home rows and states | Box events, workflow run phases | One derived state per task, served by the controller. |
| needs-you and inbox | The needs_input, run_waiting and run_failed events | A durable inbox with read state per person. |
| Timeline | Claude's session files, archived each turn and overwritten | An extractor that turns each turn into timeline entries at save time. |
| Terminal | The attach stream over the gate's WebSocket, Bearer token only | A cookie-signed-in WebSocket route and a terminal widget. |
| Turn receipt and Changes | exec can run git in the Box | A diff op that moor runs at turn end. |
| PR state | The agent mentions the PR in text | A PR link per task, with checks read from the forge. |
| Evidence | Artifacts, served with a token | Serve them to the signed-in portal. |
| Start a task | MCP start_box | A portal route that calls the same op. |
| Live updates | The Watch frame over the WebSocket, poll over HTTP | Server-sent events or the same WebSocket for the portal. |

## Look

- Dense rows, not cards. Mono for ids, branches and paths.
- One accent for "needs you", one for "working". Green and red only for diffs and checks. Errors and destructive buttons are the one exception, in the moorland rust colour.
- The existing moorland tokens stay. Dark and light both ship.
- The design system is our own CSS on those tokens, plus the browser's built-in dialog and popover elements. No Tailwind and no component library.
- The portal's security policy gains `font-src 'self'` when the fonts ship in the bundle, and `img-src 'self'` when Evidence shows images. Both land in the first portal PR that needs them.
- Keyboard first. ⌘K opens the switcher, j and k move, enter opens.

## Order

Each step ends in a check someone can rerun. The first step is useful alone.

1. **Delete the herdr front.** PR #1496.
2. **Mockup and design system.** A clickable mockup of Home and the Task page, with the reference screenshots beside it.
3. **Tasks in the portal.** The derived state, the Home list, the Task page with Timeline from events only, Evidence, and Start. Check. Start a task from the browser on the pilot fixture repo and see it reach done with a screenshot.
4. **needs you.** The waiting card, answers from the web, the inbox, and default notifications. Check. A permission prompt answered in the browser unblocks the agent.
5. **Terminal in the browser.** The signed-in WebSocket route and xterm.js. Check. Take over a working Box and type into Claude.
6. **Receipts, Changes and the PR header.** Check. A turn's receipt matches the git diff in the Box.
7. **Timeline from the transcript.** Check. Tool calls fold, and errors stay unfolded.
8. **Workflows reworked.** One repo set per task and workflow, words for schedules, plan editor, Make this recurring. Check. A workflow over two repos runs in one Box and opens a PR in the repo the agent changed.
9. **Docs.** One getting-started page per role, written against the new portal.

## Decided on 2026-11-18

1. The moor-hc bridge goes once the browser terminal ships.
2. Box events reach Claude Code over the MCP connection, and the moor plugin's local listener goes.
3. The portal says "task" to people and keeps "Box" for the computer.
4. The herdr front is removed now, before the browser terminal. It was not really used.
5. "Plan first" is offered on one-off tasks, off by default.
6. A clickable mockup of Home and the Task page comes before portal code, with the design system chosen alongside it. No Tailwind.
7. A task and a workflow each hold an ordered set of repos in one Box. The first is the starting point. This replaces one Box per repo and `also`. Decided on 2026-11-19.
8. The browser terminal uses xterm.js, and Claude stays full-screen. Scrollback is Claude's own, reached with the wheel. Decided on 2026-11-19.
