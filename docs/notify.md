# Get told when a Box idles

`moor-notify` is a front that posts a person's Box events to webhooks that
person added. It is for a receiver that does not hold an MCP stream open, such
as a CI job or a chat bot. An MCP client can use
[`subscriptions/listen`](mcp.md#get-told-instead-of-polling) on the gate
instead. The front decides nothing. The controller queues one delivery per
event and webhook, with the body built. The front claims, signs, posts and
reports. Without the front, moor queues deliveries and sends nothing.

```mermaid
sequenceDiagram
    participant C as controller
    participant N as moor-notify
    participant R as your receiver
    N->>C: webhook-claim
    C-->>N: delivery idled, url, key, body
    N->>R: POST, signed
    R-->>N: 2xx
    N->>C: webhook-report delivered
```

## Set it up

The operator enables the front once, in the controller config:

```yaml
fronts:
  notify:
    command: [moor-notify]
    webhooks: true
```

`webhooks: true` lets the front claim and report deliveries for every user. The
controller refuses both from any other front, and `moor-notify` exits with that
refusal. To run the front yourself, leave out `command` and start
`moor-notify --socket <state dir>/fronts/notify/front`. `--poll-ms` sets how
long the front waits before it claims again when nothing was queued (1000 by
default).

Each user adds their own webhooks. With the gate, call `add_webhook` with the
URL and, if you want, the event kinds. The tool shows the key once. From a
terminal, run `moor user webhook add URL [--event KIND]...`, which prints the
key once. Give the receiver the key. `list_webhooks` and `moor user webhook ls`
show a webhook's events and health, and `remove_webhook` and `moor user webhook
rm ID` remove one. A URL must be `https://`, or `http://` on `127.0.0.1`,
`[::1]` or `localhost`.

When it runs under the controller, the front's POSTs go through the front's
egress proxy. That proxy refuses private and loopback addresses, so the
receiver must be public. A front that you run yourself has no such limit.

The usual events are these; a webhook can name others that the controller
stores:

| Kind | When |
| --- | --- |
| `idled` | The agent ended its turn. |
| `setup_failed` | The Box's setup failed. |
| `session_failed` | The agent could not be started. |
| `launch_timed_out` | The agent did not start in time. |
| `relaunch_capped` | The agent ended too often, so moor does not start it again. |
| `box_failed` | The machine for the Box could not be made. |

Add `relaunched` to hear about each automatic restart of the agent, or
`closed` and `asleep` for those changes.

## What is posted

Each delivery is one `POST` with a JSON body, which the controller builds:

```json
{
  "schema": "moor.notify/1",
  "delivery": 4182,
  "event": "idled",
  "ts": 1793500000000,
  "box": "app/fix-login",
  "box_id": 57,
  "session": 91,
  "data": {},
  "lifecycle": "ready",
  "session_state": "idle",
  "agent_said": {"text": "Opened the PR.", "at": "2026-11-01T09:12:03Z"}
}
```

| Field | Meaning |
| --- | --- |
| `schema` | `moor.notify/1`. A change to the shape gets a new value. |
| `delivery` | The delivery's id. It is unique per webhook and event. |
| `event` | The event kind. |
| `ts` | When the controller stored the event, in unix milliseconds. |
| `box`, `box_id`, `session` | The Box's name and id, and the Session id. |
| `data` | The event's own fields, such as `line` for `setup_failed` or `count` and `window_secs` for `relaunch_capped`. |
| `lifecycle`, `session_state`, `agent_said` | The Box's current values, as `box_status` returns them, read just after the event. They are absent when the controller does not answer. |

The headers are these:

| Header | Value |
| --- | --- |
| `X-Moor-Event` | The event kind. |
| `X-Moor-Delivery` | The same value as `delivery`. |
| `X-Moor-Timestamp` | When this attempt was signed, in unix seconds. |
| `X-Moor-Signature` | `sha256=` and the lowercase hex HMAC-SHA256 of `<timestamp>.<body>` with the key. |

To check a POST, compute the HMAC over the timestamp header, a `.`, and the raw
body. Compare it with the header in constant time. Refuse a timestamp that is
more than a few minutes old.

```sh
printf '%s.%s' "$timestamp" "$body" | openssl dgst -sha256 -hmac "$key"
```

## Delivery

The controller hands the front at most one delivery per webhook at a time, in
the order the events happened, so a receiver sees one webhook's events in
order. Different webhooks are posted at the same time. A 2xx answer ends a
delivery. A connection failure, a timeout, 408, 429 or a 5xx gets up to 8
attempts, 2 s apart at first and doubling to at most 60 s. Any other status is
not retried. The front then reports the outcome, and the controller hands out
that webhook's next delivery. When the front gives up, it reports a short
reason such as `HTTP 410, not retried`, which `list_webhooks` shows as the
webhook's health. The reason never holds the URL or the key. The front also
writes the gave-up line to its log, with the URL's host only.

A claim is a ten minute lease. If the front stops after a POST and before its
report, or cannot reach the controller to report, the lease ends and the
controller hands the same delivery out again. The front keeps trying a report
until the controller takes it. So a delivery can arrive twice. Dedupe on
`X-Moor-Delivery`.

## Stop polling

1. Add a webhook with a URL that wakes your automation.
2. Start Boxes from MCP, Slack or another front, as before.
3. On each POST, read `event`, `box` and `agent_said`. Call `box_status` or
   `send_to_box` only for the Box in the POST.
