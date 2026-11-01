# Agent integrations

Each folder here is one installable integration for one coding agent. moor's
core stays agent-neutral; everything specific to an agent lives in its folder.

| Folder | Agent |
| --- | --- |
| `claude/` | Claude Code plugin, listed by `.claude-plugin/marketplace.json` at the repo root |

## What every integration shares

- **The gate's MCP endpoint.** The agent connects to `https://<host>/mcp` and
  signs in over OAuth. See [docs/mcp.md](../docs/mcp.md).
- **`moor-listen`.** It holds a `subscriptions/listen` stream open and prints one
  line per Box event (`moor: box <name> <event>: <what the Box said>`), plus
  `moor-listen: ...` lines about the stream and the token. It takes `--url` and
  `--token-cmd` (env `MOOR_MCP_URL`, `MOOR_TOKEN_CMD`); the command prints the
  agent's current access token.
- **A skill** that tells the agent what to do with each line.

## Add an agent

1. Create `agents/<agent>/` beside `claude/`.
2. Register the gate's `/mcp` URL in the agent's own config format. Take the URL
   from the user; never hardcode a deployment.
3. Run `moor-listen` the way the agent runs background watchers, with a token
   command that reads that agent's stored OAuth token.
4. Port the skill's table of events to the agent's instruction format.
5. Add a row to the table above.

## Claude Code

The URL comes from the `MOOR_URL` environment variable, which both `.mcp.json`
and the monitor read. Install steps are in
[docs/mcp.md](../docs/mcp.md#get-told-inside-claude-code). The listener's token
command prints `MOOR_TOKEN` when it holds a `moor_` API token, so a quiet session
is not rejected when its sign-in expires; otherwise it reads Claude Code's
stored sign-in.
