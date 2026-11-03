# Use moor on your team

This page is for a teammate on a team that already runs moor. You install
nothing. Your operator gives you the server's address, written here as
`<host>`. A repo needs no `.moor.yaml` and you need no profile. A Box starts
with neither.

## 1. Connect

From Claude Code, add the team's server:

```sh
claude mcp add --transport http moor https://<host>/mcp
```

Claude Code opens a browser. Sign in as yourself and approve the
`moor:read` and `moor:write` scopes. Every call then acts as you, so you see
and change only your own Boxes. Other agent tools connect through
`moor mcp`, as [Use your Boxes over MCP](mcp.md#use-moor-from-any-agent-tool)
shows.

From Slack, you need no setup. Mention the bot in a channel it is in, and a
coordinator agent answers in a thread.

## 2. Link your tokens

A Box needs two secrets from you: `claude` and `github`. The Proxy adds them
to the agent's requests, so neither enters the Box.

In Slack, run `/moor login` and fill in the form:

- **Claude.** Paste what `claude setup-token` prints on your own machine.
- **GitHub.** Paste what `gh auth token` prints. The token must be able to
  push to the repos you work on.

The first link makes you a moor user. If your Slack email is the one you
sign in with at `<host>`, Slack and Claude Code share that user and its
secrets. A different email makes two users.

Without this step, your first start refuses. The refusal names every missing
secret at once, so you know what to link. The details are in
[Secrets a Box uses](config.md#secrets-a-box-uses).

## 3. Start a Box

Ask your agent for the work and name the repo:

```text
Start a Box on acme/web to fix the typo on the login page.
```

The agent calls `start_box`, which returns once the Box exists. Cloning,
setup and the agent's launch carry on for a few minutes. In Slack, ask the
coordinator in the thread instead. Add the repo to the channel first with
`/moor repo add OWNER/NAME`.

## 4. Find the work

The agent works on the branch `moor/<name>` of the repo and pushes it there.
Ask it in your prompt to open a pull request when it is done. What it
publishes, such as screenshots and logs, shows as artifacts.

## 5. Watch it

- **From your agent.** `box_status` shows the lifecycle, a one-line status,
  the agent's newest message, and any sign that the Box is stuck.
  `inspect_box` shows the tail of the agent's terminal. `send_to_box` steers
  it, and `close_box` closes it.
- **In Slack.** Each Box has one status message in the thread. The front
  edits it as the Box moves from starting to working to waiting on a
  question.
- **In herdr.** Each Box is a workspace whose pane shows the agent's
  terminal. Install the plugin, name the server, and sign in:

  ```sh
  herdr plugin install IniZio/moor/crates/moor-herdr/herdr --ref main
  echo MOOR_HERDR_SERVER=https://<host> >> "$(herdr plugin config-dir moor)/env"
  ~/.local/state/herdr/plugins/moor/bin/moor-herdr login
  ```

  Then add the config snippet the install wrote, as [herdr](herdr.md#install)
  step 3 shows. A Box you start from herdr asks for missing tokens itself, so
  herdr also works in place of Slack for step 2.

A `ready` Box can still be stuck. [What a coordinator can and cannot
see](mcp.md#what-a-coordinator-can-and-cannot-see) says how to tell.

## Reference

- [Use your Boxes over MCP](mcp.md): every tool, its arguments, and the Box
  resources.
- [Slack](slack.md#how-a-thread-behaves): how a thread behaves.
- [herdr](herdr.md): the plugin and its panels.
- [Webhooks](notify.md) and [stdout lines](listen.md): get told when a Box
  needs you.
