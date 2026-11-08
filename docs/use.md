# Use a team moor

In this tutorial, you start your first Box on your team's moor and get a
pushed branch back. You need a browser, Claude Code, and the address of your
team's moor, written here as `HOST`. You install nothing else.

<!-- TODO add account page screenshots once the page ships -->


## 1. Sign in

Open `https://HOST` in your browser and sign in with your team account.

The account page opens. It shows your secrets, your machines, and the commands
that connect your tools.

## 2. Link your secrets

A Box needs two secrets from you, `claude` and `github`. The Proxy adds them to
the agent's requests, so neither enters the Box.

1. On your own computer, run `claude setup-token`. Paste what it prints into
   the **claude** field on the account page.
2. Run `gh auth token`. Paste what it prints into the **github** field. The
   token must be able to push to the repos you work on.

Both secrets now show as **Linked**. The page never shows a value again.

## 3. Connect Claude Code

Copy the Claude Code line from the account page and run it:

```sh
claude mcp add --transport http moor https://HOST/mcp
```

Start Claude Code and run `/mcp`. Claude Code opens a browser. Sign in and
approve the `moor:read` and `moor:write` scopes. The `moor` server now lists
its tools, such as `start_box` and `box_status`.

Every call acts as you, so you see and change only your own Boxes.

## 4. Start a Box

Ask Claude Code for the work, and name the repo:

```text
Start a Box on acme/web to fix the typo on the login page and open a PR.
```

Claude Code calls `start_box`. The answer names the Box and the node it runs
on. Setup carries on for a few minutes after the call returns.

## 5. Watch it and get the result

Ask Claude Code how the Box is doing. It calls `box_status`, which shows the
state, what the agent last said, and any sign that the Box is stuck.

When the agent is done, its work is on the branch `moor/ID-NAME` on GitHub.
Its last message, which `box_status` shows, names the pull request it opened.
A few minutes later the Box sleeps. To change something, ask Claude Code to
send the agent a message. The Box wakes and carries on.

When you no longer need the Box, ask Claude Code to close it.

## Next

- [Start and steer Boxes](boxes.md) covers Slack and taking over the agent's
  terminal.
- [Add your machine](add-machine.md) runs your Boxes on your own computer.
- [Set up a repo](repo.md) tells Boxes how to install and test your project.
