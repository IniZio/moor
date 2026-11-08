# Box hands

A Box is a long-lived unit of work with a stable key. Its VM is the Box's hands.
Hands can die, starve, move or be rebuilt. The Box, its sessions and its
conversation carry on in new hands.

Today one `boxes` row holds both, so a VM's fate is the Box's fate. This charter
separates them in the data model first, then builds on that.

## Model

```
Box   key = (user, repo, n) or (front, conversation, n)
 ├─ spec          set once: repo, base, brief, user, repo_config
 ├─ state         phase, wanted action, in-flight effects
 ├─ inbox         steers, delivered at least once
 ├─ hands[inc]    provider, node, handle, driver_state, workdir   one row per VM
 └─ sessions[n]   agent_session_id, run, status
Thread    bindings and its Box index, owned by the controller
Workflow  schedule and runs
Node      capacity
```

- **Incarnation.** Each hands row is a new incarnation. Ops, timers and events
  for an old incarnation cannot touch the new one.
- **Run.** Each agent launch bumps the session's run. A stale wake, relaunch,
  settle or delivery carries an old run and is dropped.
- **Wanted action.** Close, suspend and sleep are recorded as data with a fixed
  precedence. Close beats suspend, and suspend beats sleep.
- **Owner.** One per-Box task applies every mutation of that Box in order.
  Ops send it commands, and nothing else writes the Box's rows.
- **In-flight effects.** Each external step records its own phase and attempt in
  state. Events record what happened. No decision reads events back.

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
  unanswered steers, and setup and warm-rerun lookups.
- Claude's history sits in `~/.claude/projects` on the Box disk. Transcript
  archives reach the blob store but are never restored.
- The Slack front keeps its own copy of Box state, branch and node, synced by
  name.
- A steer to an asleep Box with no live session is refused. Only `start` holds
  a prompt durably.
- moor reads agent state from a hook file and pane text over ssh.
- The agent in a Box has no moor tools.

## Decisions

| Decision | Reason |
| --- | --- |
| One owner task per Box applies all its mutations | One writer per Box replaces the host flock, `intent_pid` and many write paths. |
| Split `boxes` into spec, state and hands | A VM's lifecycle stops being the Box's lifecycle. One state machine per concern. |
| Hands rows carry an incarnation | A late write for a dead VM cannot land on its replacement. |
| Sessions carry a run number | Stale agent work is dropped by number. Re-check guards can go. |
| Wanted action is a field with precedence | Racing close, suspend and sleep resolve the same way every time. |
| In-flight effects live in state, events are audit only | A decision never depends on scanning history. |
| Restore Claude's session files into new hands | `--resume` keeps the whole conversation. Replaying recent events as a prompt drops context. |
| The controller resolves a front's conversation key to a Box | Retries converge on one Box. Fronts keep a binding and an event cursor, not a copy of Box state. |
| Steers go to a per-Box inbox, deleted after delivery | A sender never waits on a wake, and a crash never loses a steer. |
| Agent events reach moor as a numbered stream through `hc` | A reader that reconnects replays what it missed. |
| claude stays in the guest under `hc` | Claude Code runs its tools where it runs. |

## Steps

Each step ends in a run on a real Box.

0. **Split the row.** New spec, state and hands tables. `driver_state` moves to
   hands, and `boxes::BoxState` goes. Proof. A Box starts, sleeps, wakes and
   closes on a real node with each concern in its own row.
1. **Box owner.** One task per Box in the controller owns its rows. Start,
   settle, wake, sleep, pressure, proxy and steer send it commands and await
   the result. The host flock and `intent_pid` go. Proof. Close, steer and
   suspend fired at one Box at once all apply in order, and a controller restart
   resumes the owner from the rows.
2. **Incarnation and portable session.** A new VM is a new hands row. The
   session's `~/.claude/projects` is archived with each transcript save and
   restored into new hands before `--resume`. Proof. Kill a Box's VM
   mid-conversation. It comes back in new hands and its next turn recalls the
   earlier turns.
3. **Run number.** Launch bumps the session's run. Wake, relaunch, settle and
   delivery carry it. Proof. Two racing relaunches start one agent. Then the
   re-check guards it covers go.
4. **Wanted action and effect state.** Replace `intent_kind` and the
   event-as-state reads with fields. Proof. A close racing a suspend ends
   closed, and the relaunch cap holds across a controller restart.
5. **Box inbox.** A steer is stored, then delivered once the agent is ready.
   Proof. Steer an asleep Box and a Box whose agent died. Each gets the message
   exactly once.
6. **Keyed Boxes, fronts without copies.** The controller resolves
   `(front, conversation, n)`. The Slack front drops its copy of Box state, and
   reads state from the controller. Proof. A Slack retry starts no second Box,
   and the thread shows the controller's state.
7. **Event stream.** `hc` forwards agent status as numbered events, and moor
   reads from its last number after a reconnect. Proof. Kill the link mid-turn,
   reconnect, and no status change is lost or repeated. Then hook-file and pane
   polling go.
8. **Box tools.** A grant scoped to the Box's repo and branch. Setup test starts
   a throwaway Box from the branch and returns the setup log. Rebox moves the
   session into new hands built from the branch's `.moor.yaml`. Proof. A Box
   breaks and fixes its `.moor.yaml`, then reboxes and keeps its conversation.

```
0 split ─▶ 1 owner ─┬─▶ 2 incarnation ─▶ 8 box tools
                    ├─▶ 3 run ─▶ 4 wanted action
                    ├─▶ 5 inbox ─▶ 6 keyed Boxes
                    └─▶ 7 event stream
```

## Open

- With more than one controller replica, the owner needs a lease per Box.
  Settle that with the controller replica work.
- Restoring `~/.claude/projects` onto new hands with the same work tree path
  gives a working `--resume`. Prove before step 1 code.

## Not in scope

- Replacing Claude Code's terminal with an agent protocol adapter.
- A new storage layer. Checkpoints and the blob store cover what is needed.
- Moving claude out of the guest.
- Running moor on another actor engine.
