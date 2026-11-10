# Events

moor records an event each time a Box changes. Every way to get events reads
the same log. [Get Box events](notify.md) shows how to receive them.

## Event kinds

A webhook that names no events gets the kinds marked with a check.

| Kind | Default | When |
| --- | --- | --- |
| `idled` | ✓ | The agent ended its turn. |
| `needs_input` | | The agent asks a question or a permission. |
| `setup_failed` | ✓ | A clone, `setup` line, or agent install failed. `data.line` names it. |
| `session_failed` | ✓ | The agent could not be started. |
| `launch_timed_out` | ✓ | The agent did not start within `box.launch_timeout`. |
| `relaunch_capped` | ✓ | The agent ended 3 times in 10 minutes, so moor stopped restarting it. |
| `box_failed` | ✓ | The Box's VM could not be made. |
| `disk_lost` | ✓ | The Box's VM was gone and a new, empty one replaced it. Work that was not pushed is lost. |
| `session_lost` | ✓ | The new VM could not get back the agent's conversation, so the agent started without it. `data.reason` says why. A prompt it was working on is not sent again. |
| `session_restored` | | The new VM got back the agent's conversation up to its last saved turn. `data.archive_at` is when that was saved. A prompt sent after then is not sent again. |
| `agent_stalled` | ✓ | The agent reported busy and sent nothing for 10 minutes. Once per stall. |
| `agent_resumed` | | A stalled agent reported again. |
| `node_disk_low` | ✓ | The Box's node fell below its free-disk floor. Once per owner. |
| `node_disk_recovered` | ✓ | That node climbed back above its floor. |
| `relaunched` | | moor restarted the agent after it ended. A start on new hands keeps the Session and emits neither this nor `transcript_skipped`. |
| `asleep`, `woke` | | The Box slept or woke. |
| `closed` | | The Box closed. `data.reason` is set when moor closed it on its own. |
| `caches_not_saved` | | A close deleted an asleep Box whose caches were not saved. |
| `exec_ran` | | `exec_in_box` or `moor exec` ran a command. |

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
| `moor: box NAME stalled: no progress for N min while DOING` | `agent_stalled`. |
| `moor: box NAME resumed after N min` | `agent_resumed`. |
| `moor: box NAME disk_lost: ...` | `disk_lost`, and `session_restored` and `session_lost` in the same shape. |
| `moor: box NAME node_disk_low: NODE has N GiB free of M GiB` | `node_disk_low`, and `node_disk_recovered` in the same shape. |
| `moor-listen: stream lost (REASON); reconnecting` | The stream dropped and the first reconnect failed. |
| `moor-listen: reconnected` | The stream is back. |
| `moor-listen: not signed in to ORIGIN; run moor login --server ORIGIN` | No sign-in. It looks again every 60 s. |
| `moor-listen: token rejected; ...` | The gate refused a fresh token. It tries again every 60 s. |
