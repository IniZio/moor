# Box hands

A Box is its identity plus its agent session. The VM is the Box's hands. Hands
can die, starve or need rebuilding. The session should carry on in new hands.

Today the session is pinned to one VM. This charter makes it portable, then
builds the pieces that depend on that.

## Today

- The store holds `agent_session_id`. Claude's own history sits in
  `~/.claude/projects` on the Box disk. Relaunch needs that disk for `--resume`.
- Transcripts reach the blob store on exit, turn, relaunch and close. Nothing
  restores them, so after `disk_lost` the conversation is gone.
- `box.node` is set once. A Box never moves to another node.
- moor learns agent state from a hook file and pane text read over ssh. Liveness
  is inferred. Unreachable for 300 s counts as ended, and busy with no hook for
  600 s counts as stalled.
- Box, session and link carry no generation. Stale work is kept out by a
  host-local Box lock, `UNIQUE(box, ordinal)` and re-checks after the lock.
- A steer to an asleep Box with no live session is refused. Only `start` holds
  a prompt durably.
- The agent in a Box has no moor tools, so it cannot test a `.moor.yaml` change.

## Decisions

| Decision | Reason |
| --- | --- |
| The session lives with the controller, not the VM | Hands can then be swapped, moved or rebuilt without losing the conversation. |
| Restore Claude's own session files, not a text replay | Replaying recent events as a prompt drops context. `--resume` keeps all of it. |
| Every agent run gets a generation | Stale relaunch, wake and sweep work is dropped by number, not by re-checks. |
| Steers go to a durable inbox per Box | A sender never waits on a wake and never gets refused for a sleeping Box. |
| Agent events reach moor as a sequenced stream through `hc` | Exit and status arrive as events. A reader that reconnects replays what it missed. |
| Box tools use grants that cannot widen | A Box can act on its own repo and branch only. |
| claude stays in the guest under `hc` | Claude Code runs its tools where it runs. |

## Steps

Each step ends in a run on a real Box.

1. **Portable session.** Archive `~/.claude/projects` for the session with each
   transcript save. Restore it onto a fresh VM before launch, then `--resume`.
   Proof. Close a Box's VM mid-conversation, start it on a new VM, and its next
   turn recalls the earlier turns.
2. **Box inbox.** A steer is stored, then delivered once the agent is ready.
   Delivery is at least once, keyed by message id. Proof. Steer an asleep Box
   and a Box whose agent died. Both get the message exactly once.
3. **Run generation.** Each launch bumps a generation on the session. Wake,
   relaunch, settle and delivery carry it, and a stale one is dropped. Proof.
   Two racing relaunches start one agent. Then the re-check guards it covers go.
4. **Event stream.** `hc` forwards the agent's status as numbered events. moor
   reads from its last number after a reconnect. Proof. Kill the link mid-turn,
   reconnect, and no status change is lost or repeated. Then hook-file and pane
   polling go.
5. **Box tools.** A grant scoped to the Box's repo and branch. Setup test
   starts a throwaway Box from the branch and returns the setup log. Rebox moves
   the session into hands built from the branch's `.moor.yaml`. Proof. A Box breaks and fixes its `.moor.yaml`, then reboxes
   and keeps its conversation.

```
1 portable session ──▶ 5 rebox, node moves, disk_lost recovery
2 inbox         ┐
3 generation    ├ independent, any order
4 event stream  ┘
```

## Not in scope

- Replacing Claude Code's terminal with an agent protocol adapter.
- A new storage layer. Checkpoints and the blob store cover what is needed.
- Moving claude out of the guest.

## To prove before step 1 code

- Restoring `~/.claude/projects` onto a new VM with the same work tree path
  gives a working `--resume`.
