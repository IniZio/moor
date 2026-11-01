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
| `moor:read` | `list_boxes`, `box_status`, `list_webhooks`, the Box resources, `subscriptions/listen` |
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
| `add_webhook` | `url` (https); optional `events` |
| `list_webhooks` | none |
| `remove_webhook` | `id` |

`close_box` refuses a Box that holds unpushed commits or uncommitted files, or
that could not be checked for them. `force: true` closes it anyway and discards
that work, the same as `moor close --force`. It is false by default.

A webhook receives signed POSTs of your Box events. `add_webhook` returns the
webhook's `id` and its HMAC `key`, shown only once; `list_webhooks` shows each
webhook's url, events and delivery health, never the key. See
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
