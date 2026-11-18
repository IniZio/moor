# Start and steer Boxes

You can start, watch, and steer a Box from Claude Code or the web portal. Each
front shows the same Box, so you can start in one and follow in another.

Before you start, link your `claude` and `github` secrets on the account page
at `https://HOST`. A start without the secrets is refused, and the refusal names each one that is
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

## Close a Box

Close a Box from any front. A close is refused while the Box holds commits
that are not pushed or files that are not committed. Ask the agent to push
first, or close with `force` to discard that work.

An asleep Box stays in every list until someone closes it.
