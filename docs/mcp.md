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

## Sign in once

Local tools that talk to moor share one sign-in per server. Run

```sh
moor login --server https://<host>
```

once. The herdr plugin, the event listener and your own scripts then use that
sign-in, and its access token refreshes on its own. A script gets a current
token from `moor token --server https://<host>`. Claude Code's
`headersHelper` can run `moor token --server https://<host> --headers`.
`moor status` shows who is signed in. `moor logout` forgets the sign-in and
revokes it, so no copy of it keeps working.

## Use moor from any agent tool

Agent tools differ in how they sign in to a remote MCP server, but every one of
them runs local stdio servers. `moor mcp` is one. It relays each message to
`https://<host>/mcp` with a fresh access token from your sign-in, so the tool
never holds a token and never needs a restart when one expires. Sign in once:

```sh
moor login --server https://moor.oursky.codes
```

Then add `moor mcp` to your tool. `--server` falls back to `MOOR_SERVER`. If
you are not signed in, the server still starts, and each call answers with the
`moor login` command to run. When the gate refuses a token, `moor mcp`
refreshes it once and retries the call.

The snippets below follow each tool's own documentation. None was checked by
running the tool.

Claude Code, in `.mcp.json`:

```json
{"mcpServers": {"moor": {
  "command": "moor", "args": ["mcp", "--server", "https://moor.oursky.codes"]}}}
```

Codex, in `~/.codex/config.toml`:

```toml
[mcp_servers.moor]
command = "moor"
args = ["mcp", "--server", "https://moor.oursky.codes"]
```

Cursor, in `~/.cursor/mcp.json` or `.cursor/mcp.json`:

```json
{"mcpServers": {"moor": {
  "command": "moor", "args": ["mcp", "--server", "https://moor.oursky.codes"]}}}
```

opencode, in `opencode.json`:

```json
{"mcp": {"moor": {"type": "local", "enabled": true,
  "command": ["moor", "mcp", "--server", "https://moor.oursky.codes"]}}}
```

pi has no MCP support of its own. Install `pi-mcp-adapter` and give it the same
`mcpServers` entry as Cursor.

Claude Code can also reach the gate directly over HTTP, running `moor token`
for a fresh header before it connects:

```json
{"mcpServers": {"moor": {"type": "http",
  "url": "https://moor.oursky.codes/mcp",
  "headersHelper": "moor token --server https://moor.oursky.codes --headers"}}}
```

## Scopes

| Scope | Allows |
| --- | --- |
| `moor:read` | `list_boxes`, `box_status`, `inspect_box`, `list_webhooks`, `list_events`, the Box resources, `subscriptions/listen` |
| `moor:write` | `start_box`, `send_to_box`, `close_box`, `retry_box`, `close_failed_boxes`, `add_webhook`, `remove_webhook` |

A token without `moor:write` gets an error from the write tools and can still
read.

## Tools

| Tool | Arguments |
| --- | --- |
| `list_boxes` | none |
| `start_box` | `repo` as `owner/name`; optional `also` (more repos, each `owner/name`), `name`, `prompt`, `base`, `node`, `wait` |
| `box_status` | `box` |
| `inspect_box` | `box`; optional `lines`, `said` |
| `send_to_box` | `box`, `text` |
| `close_box` | `box`; optional `force` |
| `retry_box` | `box` |
| `close_failed_boxes` | optional `force` |
| `add_webhook` | `url` (https); optional `events`, `header` (such as `Authorization`) |
| `list_webhooks` | none |
| `list_events` | optional `after`, `kinds`, `box`, `limit`, `wait_seconds` |
| `remove_webhook` | `id` |

`close_box` refuses a Box that holds unpushed commits or uncommitted files, or
that could not be checked for them. `force: true` closes it anyway and discards
that work, the same as `moor close --force`. It is false by default. Like
`moor close --force`, `force: true` also interrupts a start that is running the
Box's setup, so a stuck setup does not keep the Box open.

`retry_box` re-runs setup on a Box whose setup failed, the same as `moor retry`.
It refuses a Box in any other state. It returns when the retry finishes; follow
it with `box_status`.

`close_failed_boxes` closes every one of your Boxes whose setup failed, the same
as `moor close-failed`. It answers with the Boxes it closed and, for each Box it
could not close, why. `force: true` closes a Box that holds unpushed work too.
It is false by default.

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
- `stuck`: `{kind, since, detail}` when the Box shows a sign of being stuck,
  else `null`. `kind` is `stalled`, `egress_refused`, `egress_aborted`,
  `credential_withheld`, `proxy_credential_failed`, `agent_rate_limited`,
  `agent_unreachable` or `agent_link_lost`. `since` is when it began and
  `detail` says what happened in one line. A later `agent_resumed`,
  `turn_done`, `idled`, `needs_input`, new session, link or credential
  recovery, or close clears it. While it is set, `status` ends with
  `· stuck: <kind>`, as in `ready · stuck: stalled`.
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

## What a coordinator can and cannot see

A lifecycle of `ready` and a stream of events are not proof that an agent is
making progress. A `ready` Box can sit on a hung tool call, a refused request
or an unanswered question for hours. Use the tools below to judge it, and ask
the agent when they do not settle it.

| Tool | What it shows |
| --- | --- |
| `box_status` | The lifecycle, the `status` line, `stuck`, the agent's newest message, the last five raw events, artifacts and Reports. |
| `list_events` | Every event of your Boxes from an offset you keep, including `agent_stalled` and the egress and credential events. |
| `inspect_box` | The tail of the agent's terminal, the agent's recent messages, its state and `stuck`. |
| Artifacts | What the agent published: screenshots, logs and files. |

`inspect_box` takes `box` and optionally `lines` (40 by default, at most 200)
and `said` (5 by default, at most 20). It returns `pane`, the last `lines` lines
of the agent's terminal with a `status` of `live`, `exited` or `gone`. It also
returns `said`, the agent's newest messages with the oldest first, `agent` as
`list_boxes` shows it, and `stuck`. It reads only. It starts nothing, steers
nothing and changes nothing about the Box, so you can call it as often as you
like. It works on a Box you may act on and answers as if any other did not exist.

The stall signal comes from the agent's hooks. Claude Code writes one on every
tool call, prompt and stop. When the hooks say the agent is busy and nothing
has been written for ten minutes, with no `moor run` in flight, moor records
`agent_stalled` once, with what the agent was doing. When a hook writes again,
it records `agent_resumed`. moor notices at the next read of the Box, which is
a `list_boxes`, a listen stream or a webhook that watches it. An agent
without hooks never stalls by this rule. A long turn that makes tool calls is
not stalled, and a tool call that runs for more than ten minutes is: look at
`doing` in the event, and at the pane, before you act.

The pane is a snapshot of the terminal, not the transcript. It shows what is on
screen now, so a long output may have scrolled away, and a full-screen program
fills it with drawing. It is the best way to see a prompt the agent waits on.

Nothing here shows the environment of the host, the Proxy's own log, the
agent's private reasoning or the Boxes of other users. The egress and
credential events say that a request was refused or withheld, and for which
host and rule, but never carry a token, a path, a query or a body.

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
| `agent_stalled`, `agent_resumed` | `notifications/resources/updated` for its URI |
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
to the gate. For each Box it starts or is asked to watch, it prints a line in the
session when that Box idles, needs input or fails.

```sh
cargo install --git https://github.com/IniZio/moor moor
moor login --server https://<host>       # once; opens the browser
export MOOR_SERVER=https://<host>        # before starting claude, or in settings env
```

Then, inside Claude Code:

```text
/plugin marketplace add IniZio/moor
/plugin install moor@moor
```

The plugin starts `moor mcp` for its tools. When Claude starts a Box, or you ask
it to watch one, it runs `moor listen --box <name>` in the background; sessions
that never start a Box get no Box lines. Both use the one sign-in and renew it themselves, so nothing goes stale while
the session is quiet. There is no sign-in step in `/mcp`.

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
