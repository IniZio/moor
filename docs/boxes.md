# Start and steer Boxes

You can start, watch, and steer a Box from Claude Code or from Slack. To type
into the agent's own terminal, take it over in herdr. Each front shows the same
Box, so you can start in one and follow in another.

Before you start, link your `claude` and `github` secrets on the account page
at `https://HOST`. In Slack, `/moor login` replies with a link to that page. A
start without the secrets is refused, and the refusal names each one that is
missing.

## From Claude Code

Connect once, with the line from the account page:

```sh
claude mcp add --transport http moor https://HOST/mcp
```

Then ask in plain words. Claude Code picks the tool.

| To | Ask for | Tool |
| --- | --- | --- |
| Start a Box | "Start a Box on acme/web to fix the login typo." | `start_box` |
| Pick where it runs | "List my nodes." Then name one in the start. | `list_nodes` |
| Check on it | "How is fix-typo doing?" | `box_status` |
| See its terminal | "Show me fix-typo's screen." | `inspect_box` |
| Steer it | "Tell fix-typo to also update the test." | `send_to_box` |
| Answer its question | "Answer fix-typo with option 2." | `send_to_box` |
| Retry a failed setup | "Retry fix-typo." | `retry_box` |
| Close it | "Close fix-typo." | `close_box` |

`start_box` returns once the Box exists. Setup carries on, so follow it with
`box_status` until the lifecycle is `ready`.

A `ready` Box can still be stuck. `box_status` sets `stuck` when the agent has
made no progress for ten minutes, or when the Proxy refused or withheld a
request. Look at the screen with `inspect_box` before you steer.

To have Claude Code tell you when a Box needs you, install the moor plugin.
[Get Box events](notify.md#in-claude-code) shows how.

## From Slack

1. Invite the bot to a channel with `/invite @moor`.
2. Add a repo to the channel with `/moor repo add OWNER/NAME`.
3. Mention the bot at the top level and describe the work.

The coordinator answers in a thread and starts Boxes when you ask for work.
Reply in the thread to steer, ask, or close. Only the person who started the
thread directs the coordinator.

Each Box has one status message in the thread. The bot edits it as the Box
moves through starting, setting up, working, waiting on a question, asleep,
and closing.

- While the Box works, a `now:` line shows what the agent is doing.
- A **Stop** button interrupts the agent.
- A question from the agent shows one button per option.
- After 20 minutes in one turn, the bot posts a new message, so Slack notifies
  you.
- Files the Box publishes, such as screenshots, are uploaded to the thread.

[Fronts](fronts.md#slack-commands) lists every `/moor` command.

## Take over the agent's terminal in herdr

The agent's session in a Box is a real Claude Code session. Taking over means
typing into it. Today you do that with the moor plugin for
[herdr](https://herdr.dev), which shows each Box as a herdr workspace.

1. Install the plugin and name your moor:

   ```sh
   herdr plugin install IniZio/moor/crates/moor-herdr/herdr --ref main
   echo MOOR_HERDR_SERVER=https://HOST >> "$(herdr plugin config-dir moor)/env"
   ```

2. Sign in. The command opens your browser.

   ```sh
   ~/.local/state/herdr/plugins/moor/bin/moor-herdr login
   ```

3. Add the plugin's config snippet and reload herdr:

   ```sh
   cat "$(herdr plugin config-dir moor)/herdr-setup.toml" >> ~/.config/herdr/config.toml
   herdr server reload-config
   ```

Each open Box is now a workspace labelled `◈ repo/name`. Its pane is the
agent's terminal.

| Key | Does |
| --- | --- |
| `prefix+c` | New work, in a local worktree or a new Box |
| `prefix+m` | The Boxes panel, with attach, wake, and close buttons |
| `ctrl+]` | Detach. The agent keeps running. |

A new tab in a Box's workspace opens a shell in the Box, in its work tree.
`herdr plugin update moor` installs the newest release of the plugin.

## Close a Box

Close a Box from any front. A close is refused while the Box holds commits
that are not pushed or files that are not committed. Ask the agent to push
first, or close with `force` to discard that work.

An asleep Box stays in every list until someone closes it.
