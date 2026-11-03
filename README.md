# moor

moor runs coding agents in Boxes, one piece of work per Box, each on its own
machine and branch of a GitHub repo. The agent never holds your Claude or
GitHub token, because a Proxy on the host adds each token to its requests.

Pick the door that matches you.

## Use it

You are on a team that already runs moor. You install nothing. Your operator
gives you the server's address, written here as `<host>`.

1. Connect Claude Code to the team's server:

   ```sh
   claude mcp add --transport http moor https://<host>/mcp
   ```

   Claude Code opens a browser. Sign in as yourself and approve the scopes.
   If your team uses Slack, you can instead run `/moor login` in a channel
   the bot is in and mention the bot.

2. Link your tokens. Run `/moor login` in Slack and paste the output of
   `claude setup-token` and `gh auth token`. Your first start refuses until
   both the `claude` and `github` secrets are stored, and the refusal names
   each one that is missing.

3. Ask the agent to start a Box on a repo, such as "start a Box on
   `acme/web` to fix the login typo". It calls `start_box`. The repo needs no
   `.moor.yaml` and you need no profile.

4. Watch it. `box_status` shows what the Box is doing and what the agent last
   said. The agent works on the branch `moor/<name>` and pushes it there.

Full steps, and how to watch Boxes in Slack or herdr:
[Use moor on your team](docs/use.md).

## Run it

You run moor for a team on one Linux host.

1. Install the release binaries:

   ```sh
   curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | sh
   ```

2. Start the controller, which owns the state and starts Boxes, and the gate,
   which serves `https://<host>/mcp` behind your sign-in provider.
3. Give the controller a node: a place Boxes run, backed by a crabbox
   provider.
4. Enroll the first user and store their `claude` and `github` secrets.
5. Hand out `https://<host>/mcp`.

On Kubernetes, the Helm chart runs the controller and the gate from the
release image instead.

Full steps: [Run moor for a team](docs/run.md).

## Hack on it

You change moor itself.

1. Install the Rust toolchain that `rust-toolchain.toml` pins, then the
   pinned tools:

   ```sh
   make setup
   ```

2. Check and test:

   ```sh
   make lint
   make test
   ```

The crate layout, the checks, and how to run a Box from a checkout are in
[Development](docs/development.md).

## Concepts

- **Box.** One piece of work: a branch, a machine, and an agent on it.
- **Proxy.** One per Box, on the host. It adds your stored token to each
  request the Box makes, so the token never enters the Box.
- **Controller.** The long-running process that owns the state and starts,
  steers and closes Boxes.
- **Gate.** The HTTPS front that signs people in and serves the MCP endpoint.
- **Node.** A place that runs Boxes, such as a host with a crabbox provider.
- **Profile.** Your own agent settings, copied into each Box you start. It is
  optional.
- **`.moor.yaml`.** A repo's own Box settings, such as its setup command. It
  is optional.

## Documentation

For people who use moor:

- [Use moor on your team](docs/use.md): connect, link tokens, start and watch
  a Box.
- [Use your Boxes over MCP](docs/mcp.md): the tools, scopes and Box
  resources.
- [Slack](docs/slack.md): the Slack front, from both sides.
- [herdr](docs/herdr.md): see your Boxes as herdr workspaces.
- [Webhooks](docs/notify.md) and [stdout lines](docs/listen.md): get told
  when a Box needs you.

For operators:

- [Run moor for a team](docs/run.md): one host, end to end.
- [Deploy with systemd](docs/deploy.md): socket units and upgrades.
- [Configuration reference](docs/config.md): the user config, `.moor.yaml`,
  the controller config, and the vault key.
- [Nodes](docs/nodes.md), [providers and the Proxy](docs/providers.md), and
  [machines](docs/machines.md): where Boxes run.
- [Web front](docs/web.md): a local web page for the controller.

For developers:

- [Development](docs/development.md): crates, checks, and running a Box from
  a checkout.
- [Command reference](docs/cli.md): every `moor` command. The CLI is a dev
  tool, not the main user interface.
