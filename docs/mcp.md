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
| `start_box` | `repo` as `owner/name`; optional `name`, `prompt`, `base` |
| `box_status` | `box` |
| `send_to_box` | `box`, `text` |
| `close_box` | `box` |

## Operator setup

Start the gate with `--mcp-resource-url https://<host>/mcp` (or
`MOOR_GATE_MCP_RESOURCE_URL`). Without it `/mcp` is off. The sign-in provider
must issue access tokens for that URL as the audience, with the scopes above, and
must register clients on its own; the gate does not. The gate answers
`/.well-known/oauth-protected-resource` with the provider and the scopes, so a
client finds its way without configuration.
