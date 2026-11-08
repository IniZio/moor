# Box hands

A Box is a long-lived unit of work with a stable key. Its VM is the Box's hands.
Hands can die, starve, move or be rebuilt. The Box, its sessions and its
conversation carry on in new hands.

Today one `boxes` row holds both, so a VM's fate is the Box's fate. This charter
separates them in the data model first, then builds on that.

## Model

```
Box   key = (user, repo, n) or (front, conversation, n)
 ├─ spec          set once: repo, base, brief, user
 ├─ state         phase, wanted action, in-flight effects
 ├─ inbox         steers, delivered at least once
 ├─ hands[inc]    provider, node, handle, observed state, workdir, config text
 └─ sessions[n]   agent_session_id, run, status
Thread    bindings and its Box index, owned by the controller
Workflow  schedule and runs
Node      capacity
```

- **Incarnation.** Each hands row is a new incarnation. Ops, timers and events
  for an old incarnation cannot touch the new one.
- **Observed state.** A hands row holds the VM state moor last saw, with its
  time. While a node is away, sleep and pressure are decided from that record.
- **Config text.** A hands row holds the `.moor.yaml` it was built from. The
  Box's spec never changes, so a rebuilt Box is new hands under the same spec.
- **Run.** Each agent launch bumps the session's run. A stale wake, relaunch,
  settle or delivery carries an old run and is dropped.
- **Wanted action.** Close, suspend and sleep are recorded as data with a fixed
  precedence. Close beats suspend, and suspend beats sleep.
- **Owner.** One per-Box task applies every mutation of that Box in order.
  Ops send it commands, and nothing else writes the Box's rows.
- **In-flight effects.** Each external step records its own phase and attempt in
  state. Each effect also says whether a restart reruns it or reports it as
  interrupted. Events record what happened. No decision reads events back.

## Today

- `boxes` mixes spec, lifecycle, controller intent, VM runtime and placement.
  It carries two state machines, `driver_state` and `state`.
- A replacement VM after `disk_lost` reuses the same row. Nothing tells writes
  for the old VM from writes for the new one.
- Box, session and link carry no generation. A host-local Box lock,
  `UNIQUE(box, ordinal)` and re-checks after the lock keep stale work out.
- `intent_kind` with a pid records which process owns an op, not which action
  is wanted.
- Several decisions read events back as state. Examples are the relaunch cap,
  unanswered steers, stall and wake checks, and a workflow run's setup failure
  cause.
- A Box stores the `.moor.yaml` text it was started from and reads that copy.
- Claude's history sits in `~/.claude/projects` on the Box disk. Transcript
  archives reach the blob store after each turn but are never restored.
- The Slack front keeps its own copy of Box state, branch and node, synced by
  name.
- A steer to an asleep Box with no live session is refused. Only `start` holds
  a prompt durably.
- moor reads agent state from a hook file and pane text over ssh.
- The agent in a Box reaches machine tools and the user's MCP servers, but no
  moor tool. It cannot test a setup or rebuild its own Box.

## Decisions

| Decision | Reason |
| --- | --- |
| One owner task per Box applies all its mutations | One writer per Box replaces the host flock, `intent_pid` and many write paths. |
| Split `boxes` into spec, state and hands | A VM's lifecycle stops being the Box's lifecycle. One state machine per concern. |
| Hands rows carry an incarnation | A late write for a dead VM cannot land on its replacement. |
| A hands row records observed VM state with its time | moor decides while a node is away. A live read of the provider answers nothing then. |
| A hands row holds its config text, the spec does not | A rebuild from a changed `.moor.yaml` is new hands, not a new Box. |
| Sessions carry a run number | Stale agent work is dropped by number. Re-check guards can go. |
| Wanted action is a field with precedence | Racing close, suspend and sleep resolve the same way every time. |
| In-flight effects live in state, events are audit only | A decision never depends on scanning history. |
| Each effect names whether a restart reruns it | Acquire and a setup attach rerun. An exec or a prompt send is reported, never repeated. |
| Restore Claude's session files into new hands | `--resume` keeps the whole conversation. Replaying recent events as a prompt drops context. |
| The controller resolves a front's conversation key to a Box | Retries converge on one Box. Fronts keep a binding and an event cursor, not a copy of Box state. |
| Steers go to a per-Box inbox, deleted after delivery | A sender never waits on a wake, and a crash never loses a steer. |
| Agent events reach moor as a numbered stream through `hc` | A reader that reconnects replays what it missed. |
| claude stays in the guest under `hc` | Claude Code runs its tools where it runs. |

## Steps

Each step ends in a run on a real Box.

0. **Split the row.** New spec, state and hands tables. `driver_state` moves to
   hands as observed state, and `boxes::BoxState` goes. Proof. A Box starts,
   sleeps, wakes and closes on a real node with each concern in its own row.
1. **Box owner.** One task per Box in the controller owns its rows. Start,
   settle, wake, sleep, pressure, proxy and steer send it commands and await
   the result. The host flock and `intent_pid` go. Proof. Close, steer and
   suspend fired at one Box at once all apply in order, and a controller restart
   resumes the owner from the rows.
2. **Incarnation and portable session.** A new VM is a new hands row. The
   session's `~/.claude/projects` is archived with each transcript save and
   restored into new hands before `--resume`. The save runs after each turn, so
   a VM lost mid-turn loses at most that turn. Proof. Kill a Box's VM
   mid-conversation. It comes back in new hands and its next turn recalls the
   earlier turns.
3. **Run number.** Launch bumps the session's run. Wake, relaunch, settle and
   delivery carry it. Proof. A settle and a delivery for run n arrive after run
   n+1 launched, and both are dropped. Then the re-check guards it covers go.
4. **Wanted action and effect state.** Replace `intent_kind` and the
   event-as-state reads with fields. The interrupted-start pass at controller
   start reads effect state, so the handover in the controller replicas spec
   keeps working. Proof. A close racing a suspend ends closed, and the relaunch
   cap holds with the Box's events deleted.
5. **Box inbox.** A steer is stored, then delivered once the agent is ready.
   Proof. Steer an asleep Box and a Box whose agent died. Each gets the
   message, and a crash between send and delete repeats it at most once.
6. **Keyed Boxes, fronts without copies.** The controller resolves
   `(front, conversation, n)` under a unique index. The Slack front drops its
   copy of Box state, branch and node, and reads state from the controller's
   event cursor. `slack.db` keeps the thread binding and the status message.
   Proof. A Slack retry starts no second Box, and the thread shows the
   controller's state.
7. **Event stream.** `hc` forwards Claude Code's own status reports as numbered
   events, and moor reads from its last number after a reconnect. Numbering and
   replay are a change in `hc`, not in moor. Proof. Kill the link mid-turn,
   reconnect, and no status change is lost or repeated. Then hook-file and pane
   polling go.
8. **Box tools.** A grant scoped to the Box's repo and branch. Setup test starts
   a throwaway Box from the branch and returns the setup log. Rebox moves the
   session into new hands built from the branch's `.moor.yaml`. Proof. A Box
   breaks and fixes its `.moor.yaml`, then reboxes and keeps its conversation.

```
0 split ─▶ 1 owner ─┬─▶ 2 incarnation ─▶ 8 box tools
                    ├─▶ 3 run ─▶ 4 wanted action
                    ├─▶ 5 inbox
                    ├─▶ 6 keyed Boxes
                    └─▶ 7 event stream
```

## Compared with

Rivet, ax and agentrun each run agents in sandboxes. None runs Claude Code
itself in a VM that a team steers from a thread. The table reads moor as it
stands after the steps above. A cell names the step or the code behind it.

| Dimension | moor after this charter | Rivet | ax | agentrun |
| --- | --- | --- | --- | --- |
| Identity and fencing | Key under a unique index, incarnation per hands, run per session, one owner task. Steps 1, 2, 3, 6. | Keyed ids, epoch per allocation, incarnation, effects with attempts. | Name per namespace. No fencing, no reconcile loop. | Run claim with epoch and holder, re-adopted on restart. |
| Durability of the conversation | Claude's own session files, archived each turn, restored, `--resume`. Step 2. | Transcript kept by the client. A lost sandbox gets the last events replayed as text. | None. One headless run. | Every turn and tool call committed. Not a Claude Code conversation. |
| Recovery from VM loss | New hands from the checkpoint chain, session restored, same Box. Step 2. | Volume moves with the actor. Sandboxes are not swapped. | Revert to the last snapshot. A crashed agent leaves the task running. | A container per run. No snapshot. |
| Interactivity and steering | Steer mid-turn, interrupt, inbox for an asleep Box. `ops/steer.rs`, step 5. | Prompt through the adapter, live events. | None. | A message waits for the turn to end. |
| Human in the loop | Questions posted with buttons, `needs_input` events and webhooks. `docs/slack.md`. | Permission requests over RPC. The reference app auto-accepts edits. | None. | Needs-input webhooks. Workflows cannot wait on a person. |
| Real Claude Code fidelity | Claude Code in its terminal under `hc`, with the profile's plugins and the user's MCP servers. `docs/config.md`. | Driven through an agent protocol adapter. | `claude --print`. | The SDK with its native tools off. |
| Warm start | Checkpoint chain per user, repo, machine class and cache plan. `docs/nodes.md`. | Forks from database checkpoints. No sandbox warm start. | Snapshot restore. No warm pool. | None. |
| Workflows and triggers | Cron with a floor, latest missed due fired once, skip while active, breaker after three refused or timed-out runs. `docs/workflows.md`. | Schedules per actor. A failed run is not retried. | None. | Cron, at, every and signed webhooks. Catch-up window, pause after five, cost cap. |
| Multi-user team model | Users, per-user secrets, machines and workflows. A run acts as its user. | Grants per resource. The reference app shares keys. | No sign-in. | One owner. |
| Secrets and egress | The Proxy injects a secret per rule, so the Box never holds it. Egress goes through the Proxy, allowed per rule and host. `docs/config.md`. | Engine credentials stripped. Sandbox egress is not guarded. | Plaintext in the task spec. | A loopback proxy holds the keys. Egress to registries and GitHub. |
| Cost control | Awake limit, autosleep, capacity slots, cache budget. No spend cap. | None. | None. | Spend cap at the model proxy, paused at the cap. |
| Rolling deploys | One controller, gate holds requests, runner relink measured at 0 to 1 s. Forward-only migrations. | Version families, paced drain, N-1 reads and writes. | Single replica. | Single server. |
| Observability | Events per Box with an `after` cursor, listen, webhook outbox, inspect with the pane. Step 7 adds the agent's stream. | Live events, no replay. | None. Read a file over ssh. | Timeline with a cursor, webhook outbox. |

Where moor is not ahead.

- **Cost control.** agentrun meters dollars at its model proxy. moor runs on
  each user's subscription token, stored as the user secret `claude`. Users
  enter their Boxes to steer, so a subscription fits. A per-request
  dollar cap would cap nothing. Compute is the metered cost, and the awake
  limit and autosleep bound it. moor adds no spend cap.
- **Triggers.** agentrun fires on signed inbound webhooks. moor takes outside
  events through a front, which is where a team already meets its Boxes.
- **Rolling deploys.** Rivet runs two versions at once with N-1 reads and
  writes. moor runs one controller behind a gate that holds requests, and
  migrates forward only. A roll is a held gap, not a mixed fleet.
- **Durability granularity.** agentrun commits each tool call because it owns
  the loop. moor does not own Claude Code's loop, so its bound is one turn.

Rejected from the peers.

- Reading VM state live on every request, as ax does. moor must decide while a
  node is away.
- Replaying events as a prompt after sandbox loss, as Rivet's agent does.
  `--resume` on restored files keeps the context it drops.
- A durable journal that admits each effect before dispatch, as agentrun does.
  Effect state in the owner's row is that journal for the few effects a Box has.
- Capability negotiation across agent harnesses. moor runs one.
- Coordinator actors that index their children. The store is relational, so
  a query is the index.
- Hashed keyed ids. A unique index on the key gives the same convergence.

## Open

- Today one controller runs under a flock, and the controller replicas spec
  plans a lease. Owner tasks live in the one active controller and need no lease
  of their own. A second active controller would need one per Box.
- Restoring `~/.claude/projects` onto new hands with the same work tree path
  gives a working `--resume`. Prove before any step's code.

## Not in scope

- Replacing Claude Code's terminal with an agent protocol adapter.
- A new storage layer. Checkpoints and the blob store cover what is needed.
- Moving claude out of the guest.
- Running moor on another actor engine.
