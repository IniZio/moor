# Use your Boxes from an agent over MCP

`moor-gate` serves an MCP endpoint at `https://<host>/mcp`. An agent that you
sign in for can list, start, steer, inspect and close your Boxes. It never needs
the moor CLI or a copy of your credentials.

## Connect

1. In your agent's MCP settings, add a remote server with the URL
   `https://<host>/mcp`.
2. The agent asks the gate for access, gets pointed at the sign-in provider,
   and opens a browser. Sign in as yourself.
3. Approve the scopes below. The agent now lists the tools.

Every call acts as the person who signed in, so an agent sees and changes only
that person's Boxes.

## Scopes

| Scope | Allows |
| --- | --- |
| `moor:read` | `list_boxes`, `box_status`, `list_webhooks`, `list_events`, the Box resources, `subscriptions/listen` |
| `moor:write` | `start_box`, `send_to_box`, `close_box`, `add_webhook`, `remove_webhook` |

A token without `moor:write` gets an error from the write tools and can still
read.

## Tools

| Tool | Arguments |
| --- | --- |
| `list_boxes` | none |
| `start_box` | `repo` as `owner/name`; optional `name`, `prompt`, `base`, `node`, `wait` |
| `box_status` | `box` |
| `send_to_box` | `box`, `text` |
| `close_box` | `box`; optional `force` |
| `add_webhook` | `url` (https); optional `events`, `header` (such as `Authorization`) |
| `list_webhooks` | none |
| `list_events` | optional `after`, `kinds`, `box`, `limit`, `wait_seconds` |
| `remove_webhook` | `id` |

`close_box` refuses a Box that holds unpushed commits or uncommitted files, or
that could not be checked for them. `force: true` closes it anyway and discards
that work, the same as `moor close --force`. It is false by default.

A webhook receives signed POSTs of your Box events. `add_webhook` returns the
webhook's `id` and a `setup_url` and never a secret. The person opens the link
in a browser to see the HMAC key and enter the value of the `header`, if one
was given; nothing is delivered until then. Without `--public-url` the gate
serves no setup page, and the tool says so instead of giving a link.
`list_webhooks` shows each webhook's url, events, state, delivery health and,
while it waits for setup, its `setup_url`, never the key. See
[Set up a webhook](notify.md#set-up-a-webhook) and
[Notifications](notify.md) for the receiving side.

`node` is a node you may use, a shared one or one of your own; omit it for your
default. See [Nodes](nodes.md).

### What `box_status` returns

`box_status` returns what has already happened and decides nothing about when
a Box is done:

- `status`: one line, the lifecycle (or why it failed or what a start is
  doing). While the agent has ended its turn but waits on background work such
  as a CI watch, it reads `ready · waiting on CI` (`waiting on background work`
  when the agent names nothing). `list_boxes` rows carry the same `status`.
- `agent_said`: `{text, at}`, the agent's newest message, however many events
  came after it; absent when it has said nothing.
- `artifacts`: the latest version of each published artifact.
- `reports`: the latest version of each Report the agent wrote.
- `events`: the last five raw events.

Artifacts and Reports are saved after each of the agent's turns, so they show
up once the turn ends.

### Starting without blocking

A start clones the repo, runs setup and launches the agent, which takes minutes.
`start_box` therefore returns as soon as the Box exists, with the same fields
`box_status` gives (name, repo, branch, lifecycle, session state). Setup carries
on in the background; poll `box_status` until the lifecycle is `ready`, or
`failed` with the reason in its events. A failure after the early return shows
only there, not in the `start_box` result.

Pass `wait: true` to block until setup and the agent launch finish and get the
controller's full start result, as `start_box` did before.

If the Box name already exists, `start_box` does not return early: it attaches
and answers as the controller does, so a start that collides with another
start of the same name gets the controller's busy error, with or without `wait`.
A start the controller refuses before the Box exists (bad repo, no capacity)
returns that error directly.

## Read events from an offset

`list_events` reads your Box events from the event log, the way a Kafka
consumer fetches. It suits a client that polls, such as a bot, and a webhook
receiver that wants to catch up after downtime.

| Argument | Meaning |
| --- | --- |
| `after` | Return events with an offset greater than this. Default 0, the start of the log. |
| `kinds`, `box` | Only these event kinds, only this Box. |
| `limit` | Most events to return. Default 50, at most 100. |
| `wait_seconds` | When nothing is past `after`, wait up to this long for an event. Default 0, at most 30. |

It returns `{"events": [...], "next": 14, "caught_up": true}`. Each event has
`offset`, `kind`, `box`, `box_id`, `session`, `ts` and `data`. Pass `next` as
`after` in the next call. `caught_up` is true when the call reached the end of
the log.

The rules, in plain words:

- You keep the offset. The server remembers nothing about you: no
  acknowledgement, no read marks, no groups. Reading changes nothing.
- The same `after` and filters return the same events in the same order, so a
  call you repeat after a crash is safe.
- Offsets increase but skip. Events of other users share the log, so the
  offsets you see have gaps. Never count on `after + 1` being the next event.
- `next` is the last offset returned. When nothing was returned it is the end
  of the log, so the next call does not scan again. It is never less than
  `after`.
- Events are retained indefinitely. Nothing deletes them today.

A polling loop:

```text
after = 0
loop:
  result = list_events(after=after, wait_seconds=30)
  handle(result.events); after = result.next   # save after before you handle the next batch
```

Every webhook body carries the event's `offset`. After downtime, call
`list_events` with `after` set to the last offset you handled, and process what
it returns before you trust new deliveries. A delivery you already handled
carries an offset you have seen, so skip it.

### Poll over HTTP

Code that is not an MCP client can read the same events with `GET /events`. It
takes the same Authgear access token as `/mcp`, with the `moor:read` scope, and
the arguments as query parameters: `after`, `kind` (repeat it for several),
`box`, `limit` and `wait_seconds`. The body is the `list_events` result.

A bot gets its token by signing in to Authgear as an OAuth client: it runs the
authorization code flow with PKCE once, asking for the scope
`openid offline_access moor:read` and the gate's `/mcp` URL as the resource. It
then trades its refresh token for a new access token, which lives about an
hour. `/.well-known/oauth-protected-resource` on the gate names the issuer.

```sh
curl -H "Authorization: Bearer $ACCESS_TOKEN" "https://<host>/events?after=0&wait_seconds=30"
```

A loop that keeps `next` in a file and prints each event:

```sh
while :; do
  r=$(curl -sf -H "Authorization: Bearer $ACCESS_TOKEN" \
    "https://<host>/events?after=$(cat next 2>/dev/null || echo 0)&wait_seconds=30") || { sleep 5; continue; }
  echo "$r" | jq -c '.events[]'; echo "$r" | jq .next > next
done
```

A missing or bad token gets 401, a token without `moor:read` gets 403, and a
bad parameter gets 400 with `{"error": "..."}`. Every webhook body carries the
event's `offset`, so a receiver can pass it as `after` to catch up.

## Get told instead of polling

Each of your open Boxes is a resource. Its URI is `moor://box/<name>`, such as
`moor://box/app/fix-login`. `resources/list` returns one entry per Box, with
the Box's one-line status as its description. `resources/read` returns the same
JSON as `box_status`, as `application/json` text.

A client on MCP 2026-07-28 sends `subscriptions/listen` once and keeps the
stream open. It needs no `initialize` and no session. Every POST, listen
included, carries these headers:

- `MCP-Protocol-Version: 2026-07-28`
- `Mcp-Method: <method>`, and `Mcp-Name: <uri>` for `resources/read`
- `Accept: application/json, text/event-stream`

and these fields in `params._meta`:

- `io.modelcontextprotocol/protocolVersion`: `"2026-07-28"`
- `io.modelcontextprotocol/clientInfo`: `{"name": ..., "version": ...}`
- `io.modelcontextprotocol/clientCapabilities`: `{}`

`initialize` still answers `2025-11-25`. Do not take that answer as proof that
listen is missing: the 2026-07-28 methods work without it. The gate sends an
SSE comment every 15 s to keep the stream alive.

```json
{"jsonrpc": "2.0", "id": 7, "method": "subscriptions/listen", "params": {
  "_meta": {
    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
    "io.modelcontextprotocol/clientInfo": {"name": "me", "version": "1"},
    "io.modelcontextprotocol/clientCapabilities": {}
  },
  "notifications": {
    "resourcesListChanged": true,
    "resourceSubscriptions": ["moor://box/app/fix-login"]
  }}}
```

The gate acknowledges the URIs it accepts, which are the `moor://box/` ones.
Then it follows your events on the controller from that moment:

| Event of a Box | Notification |
| --- | --- |
| `idled`, `busy`, `needs_input` | `notifications/resources/updated` for its URI |
| `setup_failed`, `session_failed`, `launch_timed_out`, `box_failed` | `notifications/resources/updated` for its URI |
| `relaunched`, `relaunch_capped` | `notifications/resources/updated` for its URI |
| `box_ready`, `asleep`, `woke`, `closed` | `notifications/resources/updated` for its URI |
| `started`, `closed`, `box_deleted` | `notifications/resources/list_changed` |

A notification names only the resource (`params.uri`). Read the resource to see what happened.
Its `events`, `status` and `agent_said` say it. To follow a Box that started
after the stream opened, open a second listen that names its URI.

The stream ends when the client cancels the request or disconnects. It also
ends with an error when the controller ends the watch. Events that happen while
no stream is open are not replayed. After a reconnect, list and read the Boxes
again, then listen again. A client on an older MCP version gets
`method not found`. It can poll `box_status`, or use the
[webhook front](notify.md), which keeps a cursor and posts each event to a URL.
An ingress in front of the gate must let a response stream stay open.

[`moor-listen`](listen.md) does all of this and prints a line per event.

To replace a cron that polls:

1. Call `start_box`, and note the Box name.
2. Listen on `moor://box/<name>`.
3. On each `updated`, read the resource and act on `status` and `agent_said`.

## Get told inside Claude Code

The `moor` plugin in [`agents/claude`](../agents/README.md) connects Claude Code
to the gate and prints a line in the session when a Box idles, needs input or
fails.

```sh
export MOOR_URL=https://<host>/mcp   # before starting claude
cargo install --git https://github.com/IniZio/moor moor-listen
```

Then, inside Claude Code:

```text
/plugin marketplace add IniZio/moor
/plugin install moor@moor
```

Sign in to the `moor` server from `/mcp`. The plugin reuses that sign-in for the
listener, so there is no second login. It needs `jq` on Linux. macOS Keychain
lookup is not supported yet.

## Operator setup

Start the gate with `--mcp-resource-url https://<host>/mcp` (or
`MOOR_GATE_MCP_RESOURCE_URL`). Without it `/mcp` is off. The sign-in provider
must issue access tokens for that URL as the audience, with the scopes above, and
must register clients on its own; the gate does not. The gate answers
`/.well-known/oauth-protected-resource` with the provider and the scopes, so a
client finds its way without configuration.

## Browser sign-in

Some setup, such as creating API tokens, must be done by a person in a browser,
never by an agent. Start the gate with `--public-url https://<host>` (or
`MOOR_GATE_PUBLIC_URL`) to turn on browser sign-in at `/auth/*`. Without it those
paths answer 404. Register exactly `https://<host>/auth/callback` as a redirect
URI on the gate's client (`--client-id`); the gate signs in as a public client
with PKCE and asks for the `openid email` scopes.

Open `https://<host>/auth/me` to check it. It sends you through the provider and
then shows the email you signed in as. A session lasts 15 minutes and lives only
in the gate's memory, so a restart signs everyone out. Run one gate replica: a
sign-in that starts on one replica cannot finish on another.
