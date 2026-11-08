# Box hands

A Box's agent survives heavy work in its own Box. The agent can change
`.moor.yaml` and see the result without asking a person.

A Box is its identity plus its session. Both already live with the controller.
The store holds `agent_session_id`, and the blob store holds transcripts. The VM
is the Box's hands. Hands can be starved, rebuilt or swapped while the session
carries on.

## Problems today

- The agent shares the guest with the work it starts. No cgroup, OOM score or
  CPU weight protects `hc` or claude. A build that fills guest memory can make
  the guest kernel kill the agent first.
- moor reads `.moor.yaml` from GitHub when the Box starts and keeps that copy for
  the Box's life. `retry_box` works only on a `SetupFailed` Box, and reuses the
  stored copy.
- The agent has no moor tools in the Box. It cannot test a setup change, so a
  broken `.moor.yaml` surfaces in the next teammate's Box.

## Decisions

| Decision | Reason |
| --- | --- |
| claude stays in the guest | Claude Code runs Bash, Read and Edit where it runs. Moving it out means replacing its tools and couples moor to one harness. |
| Guest slices protect the agent | Cheapest fix. Host-side suspend already handles host pressure, and nothing handles guest pressure. |
| Rebox swaps hands, keeps the session | Same Box id, thread and session. New VM from the branch's `.moor.yaml`, then `--resume`. |
| Setup test runs in a scratch Box | The agent's own session is never at risk from a broken setup. |
| Box tools reach only the Box's own repo and branch | Scoped in the Proxy, so a Box cannot start Boxes for other repos. |
| Rebox and setup test read the pushed branch | One source for `.moor.yaml`. No upload of guest files. |

## Guest slices

```
guest
├─ moor-agent.slice   hc, claude, hooks   memory.min, cpu.weight high, oom_score_adj -1000
└─ work.slice         everything the agent starts   memory.max = guest RAM - agent reserve
```

Claude's Bash children move into `work.slice` so builds and tests never count
against the agent's reserve.

Proof. stress-ng fills guest memory and CPU in a real Box. The agent still
answers a steer within 10 s. The OOM kill lands in `work.slice`. Measured
before and after on the same Box size.

## Rebox

```
agent pushes branch ─▶ rebox(box)
  controller: read .moor.yaml at branch head
  provision new VM (setup, caches, checkpoints as a start)
  restore Claude session store from latest transcript save
  launch agent with --resume, tell it the hands changed
  retire old VM
```

Rebox fails closed. If setup fails on the new VM, the old VM keeps running and
the agent hears the setup log.

Proof. A real Box edits `.moor.yaml`, pushes and reboxes. Its next turn
sees the new setup with its conversation intact.

## Setup test

`test_setup(branch)` starts a scratch Box from the branch, runs setup only,
returns pass or fail with the setup log, and closes the scratch Box. It counts
against the same capacity as any Box.

Proof. A real Box breaks `.moor.yaml` and gets the failing log back. It fixes
the file and gets a pass. The scratch Box is gone in both cases.

## Guards today

Agents already die in practice. moor answers with guards keyed on "pane gone or
silent", never on a guest OOM kill.

| Guard | With slices | With rebox |
| --- | --- | --- |
| `revive` relaunch on wake or steer | fires rarely | replaced by a swap when the guest is gone |
| 300 s unreachable counts as ended (`link::recover`) | fires rarely | replaced by a swap |
| `relaunch_capped`, 3 per 10 min | kept, crash-loop breaker | kept |
| transcript archive deadlines, 15 s and 60 s | can shrink | kept, last pull before swap |
| `agent_stalled` after 600 s | kept | candidate swap trigger |

Reattach, `await_guest`, `resume_owed`, `tell_after_suspend` and the transcript
saves stay. They handle link loss and host-pressure suspend, not agent death.

## Order

1. Guest slices. No dependency.
2. Setup test. Needs the Box tool path through the Proxy.
3. Rebox. Reuses the setup test's tool path and the transcript save.

## Settled

- Rebox reads the pushed branch only. It refuses while the Box has unpushed work.
- Agent reserve comes from the stress-ng measurement, not a guess.

## To prove before rebox code

- Restoring the Claude session store from the transcript archive gives a working
  `--resume` on a new VM.
