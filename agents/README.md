# Agent integrations

Each folder here is one installable integration for one coding agent. moor's
core stays agent-neutral; everything specific to an agent lives in its folder.

| Folder | Agent |
| --- | --- |
| `claude/` | Claude Code plugin, listed by `.claude-plugin/marketplace.json` at the repo root |

## What every integration shares

- **The gate's MCP endpoint.** The agent connects to `https://<host>/mcp` and
  signs in over OAuth. See [docs/mcp.md](../docs/mcp.md).
- **`moor listen`.** It holds a `subscriptions/listen` stream open and prints one
  line per Box event (`moor: box <name> <event>: <what the Box said>`), plus
  `moor-listen: ...` lines about the stream and the sign-in. It takes `--server`
  (env `MOOR_SERVER`) and uses the sign-in that `moor login --server ...` saved.
  Install the full `moor` binary and sign in once.
- **A skill** that tells the agent what to do with each line.

## Add an agent

1. Create `agents/<agent>/` beside `claude/`.
2. Register the gate's `/mcp` URL in the agent's own config format. Take the URL
   from the user; never hardcode a deployment.
3. Run `moor listen --server <origin>` the way the agent runs background
   watchers. The user runs `moor login --server <origin>` once.
4. Port the skill's table of events to the agent's instruction format.
5. Add a row to the table above.

## Claude Code

`.mcp.json` reads the `/mcp` URL from `MOOR_URL`. The monitor runs
`moor listen --server "$MOOR_SERVER"`, falling back to `MOOR_URL`, and needs
`moor login --server ...` run once. Install steps are in
[docs/mcp.md](../docs/mcp.md#get-told-inside-claude-code).
