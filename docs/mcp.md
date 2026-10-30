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
| `moor:read` | `list_boxes`, `box_status` |
| `moor:write` | `start_box`, `send_to_box`, `close_box` |

A token without `moor:write` gets an error from the write tools and can still
read.

## Tools

| Tool | Arguments |
| --- | --- |
| `list_boxes` | none |
| `start_box` | `repo` as `owner/name`; optional `name`, `prompt`, `base`, `wait` |
| `box_status` | `box` |
| `send_to_box` | `box`, `text` |
| `close_box` | `box` |

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

## Operator setup

Start the gate with `--mcp-resource-url https://<host>/mcp` (or
`MOOR_GATE_MCP_RESOURCE_URL`). Without it `/mcp` is off. The sign-in provider
must issue access tokens for that URL as the audience, with the scopes above, and
must register clients on its own; the gate does not. The gate answers
`/.well-known/oauth-protected-resource` with the provider and the scopes, so a
client finds its way without configuration.
