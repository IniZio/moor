# Events

moor records an event each time a Box changes. Every way to get events reads
the same log. [Get Box events](notify.md) shows how to receive them.

## Event kinds

A webhook that names no events gets the kinds marked with a check.

| Kind | Default | When |
| --- | --- | --- |
| `idled` | ✓ | The agent ended its turn. |
| `needs_input` | ✓ | The agent asks a question or a permission, at a prompt or in the reply that ends its turn. A reply that asks keeps the Box needing you even while background work the turn started still runs. The agent's `waiting_on` names that work. |
| `setup_failed` | ✓ | A clone, `setup` line, or agent install failed. `data.line` names it. |
| `session_failed` | ✓ | The agent could not be started. |
| `launch_timed_out` | ✓ | The agent did not start within `box.launch_timeout`. |
| `sleep_held` | | A Box past `box.max_awake` stays awake because its agent is working or waits on a background job. `data.reason` is `working` or `background_job`, and `data.awake_for_secs` how long the Box has been awake. Recorded once for each hold. |
| `relaunch_capped` | ✓ | The agent ended 3 times in 10 minutes, so moor stopped restarting it. |
| `death_capped` | ✓ | The Box's places died while its agent worked, so moor did not resume it on its own. The Box sleeps and `box_status` shows it `stuck`. `data.reason` says how the places ended. A wake or a message brings it back. |
| `box_failed` | ✓ | The Box's VM could not be made. |
| `disk_lost` | ✓ | The Box's VM was gone and a new, empty one replaced it. Work that was not pushed is lost. |
| `session_lost` | ✓ | The new VM could not get back the agent's conversation, so the agent started without it. `data.reason` says why. A prompt it was working on is not sent again. |
| `session_restored` | | The new VM got back the agent's conversation up to its last saved turn. `data.archive_at` is when that was saved. A prompt sent after then is not sent again. |
| `agent_stalled` | ✓ | The agent reported busy and sent nothing for 10 minutes. Once per stall. |
| `agent_resumed` | | A stalled agent reported again. |
| `node_disk_low` | ✓ | The Box's node fell below its free-disk floor. Once per owner. |
| `node_disk_recovered` | ✓ | That node climbed back above its floor. |
| `run_waiting` | ✓ | A workflow run entered an approve step. `data` has `workflow`, `repo`, `run`, `step`, `question` and `link`, the run page path `/workflows/NAME/runs/ID`. |
| `run_failed` | ✓ | A workflow run ended Failed, Incomplete, TimedOut or Refused. `data` has `workflow`, `repo`, `run`, `outcome`, `step` (absent when the run never reached one), `reason` and `link`. Delivered and Declined runs and skipped firings write no event, so success is silent. |
| `report_refused` | | moor did not accept the agent's `report_step`. `data.step` is the step or gate it reported, and `data.reason` is the reply it got. The timeline shows it as a plain item, not an error. |
| `relaunched` | | moor restarted the agent after it ended. A start on new hands keeps the Session and emits neither this nor `transcript_skipped`. |
| `asleep`, `woke` | | The Box slept or woke. |
| `closed` | | The Box closed. `data.reason` is set when moor closed it on its own. It is `run_ended`, `pr_merged`, `pr_closed`, `idle`, `setup_failed_expired` or `close_failed`. |
| `auto_close_held` | ✓ | moor's own close of the Box was due, but the Box stays open. It holds work that is not pushed, or it was not checked when it slept. `data.due` is `run_ended`, `pr_merged`, `pr_closed` or `idle`. `data.why` says what holds it. It is written once for the same reason. `box_status` shows the Box as `close_held` until it closes. |
| `close_refused` | | moor gave up a close it was waiting to finish, so the Box stays open. `data.reason` is the refusal, such as `{"refusal": "unsaved", "branch", "ahead", "dirty"}` or `{"refusal": "unchecked", "reason"}`, or null when the close failed another way. `data.error` says why in words. The event is written with the give-up itself. When the close woke an asleep Box, moor puts it back to sleep afterwards, and that shows as a later `asleep` event. |
| `caches_not_saved` | | A close deleted an asleep Box whose caches were not saved. |
| `terminal_opened` | | A person opened the Box's terminal. `data.shell` is true for a login shell. It restarts the clock that closes a run's Box. |
| `exec_ran` | | `exec_in_box` or `moor exec` ran a command. |
| `turn_changes` | | moor read from git what the agent's turns changed, once they ended. `data.turn` counts the turns ended so far. `data.repos` lists each repo, first one first, as `{repo, turn, total, files}`. `turn` and `total` are `{files, added, removed}`, for the turn and against the base. `files` lists up to 200 files as `{path, from, added, removed}`, with null counts for a binary file. |
| `diff_kept` | | moor kept the Box's whole diff against its base. It does so when a turn ends, when the Box sleeps and when it closes. The portal then shows the diff without waking the Box. moor writes the event only when the diff changed. `data.repos` lists each repo, first one first, as `{repo, total, files, patch, cut}`. `patch` is the unified diff. All repos share 1 MiB, and `cut` is true for a patch past it. A webhook gets this kind only if it was added with it. |
| `port_forward_opened` | | A person forwarded a port of the Box to their computer. `data.port` is the Box's port and `data.local_port` is the one on their computer. |
| `port_forward_closed` | | The forward ended. `data.connections`, `data.bytes_in` and `data.bytes_out` count what it carried. A forward whose client vanished writes none; the Box's next `asleep` or `closed` marks the end. |
| `egress_seen` | | A Docker Box reached a host it had not reached before. `data.hosts` lists each new row and each row whose count changed, as `{host, port, path, first_seen, count}`. `path` is `proxied`, `credential` or `direct`, and a `direct` row's `host` is an IP address. A changed count alone waits up to 10 minutes. List them with `moor events --box <name> --kind egress_seen`. [Egress log](specs/egress-log.md) explains each path. |
| `machine_attached` | | A Box took a slot on one of its user's machines. This follows a claim, a wait that ended, or a wake. `data.name` is the machine and `data.slot` the slot. `data.runner` is the runner that holds the machine's link, when one does. |
| `machine_detached` | | The Box let go of its slot, because it closed, released it, or slept. `data.name` is the machine. `data.asleep` is true when the Box slept and keeps its claim. The machine's `release` hook runs after the close and records no event. |
| `port_forward_refused` | | A forward was refused for a reason other than who asked, such as nothing listening on the port. `data.port` and `data.reason` say which and why. |

A webhook keeps the list it was added with. To get a kind added later, remove
the webhook and add it again.

## Webhook deliveries

Each delivery is one `POST` with a JSON body:

```json
{
  "schema": "moor.notify/1",
  "delivery": 4182,
  "event": "idled",
  "ts": 1793500000000,
  "box": "app/fix-login",
  "box_id": 57,
  "session": 91,
  "offset": 1203,
  "data": {},
  "lifecycle": "ready",
  "session_state": "idle",
  "agent_said": {"text": "Opened the PR.", "at": "2026-11-01T09:12:03Z"}
}
```

| Header | Value |
| --- | --- |
| `X-Moor-Event` | The event kind. |
| `X-Moor-Delivery` | The delivery id, unique per webhook and event. |
| `X-Moor-Timestamp` | When this attempt was signed, in Unix seconds. |
| `X-Moor-Signature` | `sha256=` and the hex HMAC-SHA256 of `<timestamp>.<body>`. |

- One webhook gets its deliveries one at a time, in event order.
- A 2xx answer ends a delivery.
- A connection failure, a timeout, 408, 429, or a 5xx gets up to 8 attempts.
  The wait starts at 2 s and doubles to at most 60 s.
- Any other status is not retried. `list_webhooks` shows the reason, such as
  `HTTP 410, not retried`.
- A delivery can arrive twice. Skip an `X-Moor-Delivery` you have seen.
- The front posts only to public addresses, and checks each redirect too.

## moor listen lines

`moor listen` and `moor-listen` print one line per event, flushed at once.
Events from before the first connect are not printed. After a reconnect, the
listener prints the events it missed.

| Line | When |
| --- | --- |
| `moor: box NAME EVENT: SAID` | An `idled`, `needs_input`, `setup_failed`, `session_failed`, `launch_timed_out`, `box_failed`, `relaunch_capped`, or `closed` event. `SAID` is the first line of the agent's message, cut at 200 characters. |
| `moor: box NAME run_failed: WORKFLOW (REPO) OUTCOME at STEP: REASON LINK` | `run_failed`, and `run_waiting` as `run_waiting: WORKFLOW (REPO) asks: QUESTION LINK`. |
| `moor: box NAME auto_close_held: moor did not close it, WHY; push or discard that, then close it` | `auto_close_held`. |
| `moor: box NAME death_capped: DETAIL` | `death_capped`. `DETAIL` says how the machine stopped. |
| `moor: box NAME stalled: no progress for N min while DOING` | `agent_stalled`. |
| `moor: box NAME resumed after N min` | `agent_resumed`. |
| `moor: box NAME disk_lost: ...` | `disk_lost`, and `session_restored` and `session_lost` in the same shape. |
| `moor: box NAME node_disk_low: NODE has N GiB free of M GiB` | `node_disk_low`, and `node_disk_recovered` in the same shape. |
| `moor-listen: stream lost (REASON); reconnecting` | The stream dropped and the first reconnect failed. |
| `moor-listen: reconnected` | The stream is back. |
| `moor-listen: not signed in to ORIGIN; run moor login --server ORIGIN` | No sign-in. It looks again every 60 s. |
| `moor-listen: token rejected; ...` | The gate refused a fresh token. It tries again every 60 s. |
