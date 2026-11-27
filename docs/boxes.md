# Claude Code and the CLI

The [portal](start.md) is the main place to work. Claude Code is the second
way in. It starts, checks and steers the same tasks with the same states, so you
can start in one and follow in the other. The tools call a task a Box.

Before you start, link your `claude` and `github` secrets in the portal's
Settings. A start without them is refused, and the refusal names each one that
is missing.

## Connect Claude Code

Copy the Claude Code line from **Settings**, then **Connect**, and run it:

```sh
claude mcp add --transport http moor https://HOST/mcp
```

Start Claude Code and run `/mcp`. Claude Code opens a browser. Sign in and
approve the `moor:read` and `moor:write` scopes. The `moor` server now lists
its tools, such as `ask` and `box_status`.

Every call acts as you, so you see and change only your own Boxes.

## Ask in plain words

Claude Code picks the tool.

| To | Ask for | Tool |
| --- | --- | --- |
| Start a task | "Ask moor to fix the login typo on acme/web." | `ask` |
| Start a Box to work in by hand | "Start a Box on acme/web." | `start_box` |
| Pick where a Box runs | "List my nodes." Then name one in the start. | `list_nodes` |
| Check on it | "How is fix-typo doing?" | `box_status` |
| See its terminal | "Show me fix-typo's screen." | `inspect_box` |
| Steer it | "Tell fix-typo to also update the test." | `send_to_box` |
| Answer its question | "Answer fix-typo with option 2." | `send_to_box` |
| Retry a failed setup | "Retry fix-typo." | `retry_box` |
| Close it | "Close fix-typo." | `close_box` |

`ask` returns once moor has started an agent place for the task. Its agent
reads the repos through GitHub, and starts a work Box when the task needs code
run. Follow the place with `box_status`. `start_box` returns once the Box
exists. Setup carries on, so follow it with `box_status` until the lifecycle
is `ready`. A Box from `start_box` runs no agent.

A `ready` Box can still be stuck. `box_status` sets `stuck` when the agent has
made no progress for ten minutes, or when the Proxy refused or withheld a
request. Look at the screen with `inspect_box` before you steer.

When the agent is done, its work is on the branch `moor/ID-NAME` on GitHub,
and its last message names the pull request it opened. The same Box shows in
the portal, where Changes and Evidence show the rest.

To have Claude Code tell you when a Box needs you, install the moor plugin.
[Get Box events](notify.md#in-claude-code) shows how. [MCP tools](mcp.md) lists
every tool, and [CLI](cli.md) covers the `moor` command.

## Close a Box

Close a Box from any front. A close is refused while the Box holds commits
that are not pushed or files that are not committed. Ask the agent to push
first, or close with `force` to discard that work.

An asleep Box does not wake for a close. moor records what the Box holds when it
goes to sleep. This covers each repo. The close reads that record. A Box with no
record is checked by waking it.

moor also closes Boxes nobody needs any more, once a minute:

- A workflow run's Box closes 24 hours after the run ends. Opening its terminal
  or steering it restarts the 24 hours. See
  [When a run's Box closes](workflows.md#when-a-runs-box-closes).
- A one-off Box, one that is not from a run, closes when its pull request merges or
  closes. It also closes after 7 days with no steer, terminal or finished turn.
- A Box with unpushed commits or uncommitted files is not closed. moor writes an
  `auto_close_held` event that says why. `box_status` shows the Box as
  `close_held`. The Box's line in the list ends with `not closed:` and the
  reason. A person pushes or discards the work. Then moor closes the Box, or you
  close it.
- An asleep Box whose sleep could not check its work is held the same way. The
  reason says why the check was skipped, such as memory on its node being
  critical. Wake the Box or close it yourself. Either one checks it.
- A Box whose agent is working is left alone.

The `closed` event has `reason` `run_ended`, `pr_merged`, `pr_closed` or `idle`.
A close keeps the artifacts, the transcript and the stored diff.
[Controller config](config.md#controller-config) lists the keys that set the
times. A Box has no keep flag yet.

## Open a web app in the Box from your browser

Teammates have no port forward to a Box from their own computer for now. The owner removed the old `moor-hc forward` bridge. A proper design comes later. See [Box port forward](specs/box-port-forward.md).

Until then, a step that needs you in the app, such as signing in with a second factor, has no supported path.

`moor forward` is a development tool. It needs a controller on the same computer. It has no sign-in and no `--server`.

```sh
moor forward fix-login 5177
```

`moor` listens on `localhost:5177` on that computer and carries each connection to port 5177 in the Box. Ctrl-C ends the forward.

- Pass `--local-port N` to listen on another port.
- Opening the forward wakes a sleeping Box. A browser tab that retries never does.
- Traffic in the last 10 minutes keeps the Box awake. An open forward with no traffic does not.
- Only the Box's own user can forward. The Box must run on a node.
