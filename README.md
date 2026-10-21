# moor

moor runs coding agents in Boxes. A Box is one piece of work. It has a
git worktree and branch on your host, a herdr workspace that shows the agent's
terminal, and a machine from crabbox where the agent runs. The agent does
not get your Claude or GitHub token. A Proxy on your host adds each token to
the agent's requests.

moor is for engineers who run several agents at once and want each one on
its own machine and branch. People reach moor through fronts, such as the
Slack front. The `moor` CLI is a dev tool for driving and debugging the core
directly. It is not the main user interface.

## Concepts

- **Box.** One piece of work, with its own branch and machine. You create one
  with `moor start` and remove it with `moor close`.
- **Session.** One run of an agent. A Box Session runs on a Box. A host
  Session runs on the controller host with no Box, and the Boxes it starts
  each run a child Session.
- **Controller.** `moor controller serve`, a long-running process that owns
  `moor.db` and answers requests on a Unix socket.
- **Front.** A separate program that connects people to the controller over
  its socket. `moor-slack` and `moor-web` are fronts.
- **Proxy.** One per Box, on the host. The Box sends its HTTP traffic through
  the Proxy, which adds the credential that a rule names for each host.
- **Vault.** The controller encrypts each user's secrets in `moor.db` with a
  key that it keeps beside its config file.

## Start your first Box

This guide starts one Box from a git repo on your machine. You need:

- the Rust toolchain that `rust-toolchain.toml` pins
- `herdr` on `PATH`
- crabbox 0.79.1-moor.1 or a newer `-moor` build of
  [IniZio/crabbox](https://github.com/IniZio/crabbox) on `PATH`, set up for
  the `local-container` provider
- `gh`, logged in, because moor reads your GitHub token with `gh auth token`
- a Claude Code token from `claude setup-token`

1. Build the CLI and check that it runs:

   ```sh
   cargo build --release --locked -p moor
   export PATH="$PWD/target/release:$PATH"
   moor version
   ```

   The output names the version, the commit, and the envelope schema:

   ```text
   moor 0.2.0 986432dbfeb4fb9b15471793c1f607ea34108f47 schema=1
   ```

2. Tell moor where your Claude token is. Create
   `~/.config/moor/config.yaml` with this content:

   ```yaml
   credentials:
     CLAUDE_CODE_OAUTH_TOKEN: {env: CLAUDE_CODE_OAUTH_TOKEN}
   ```

   Then set that variable in the shell that runs moor:

   ```sh
   export CLAUDE_CODE_OAUTH_TOKEN='<the token>'
   ```

3. In your repo, preview the settings a Box would get:

   ```sh
   cd ~/src/your-repo
   moor env
   ```

   Each line names a setting and the layer it came from, such as
   `box provider  [default]  local-container`.

4. Start a Box with a goal for the agent:

   ```sh
   moor start fix-typo --goal "Fix the typo in the README"
   ```

   moor creates the worktree, the herdr workspace, and the machine, and then
   starts the agent.

5. Follow the Box, and close it when the agent is done:

   ```sh
   moor show fix-typo
   moor wait fix-typo
   moor close fix-typo
   ```

   `moor close` refuses while the Box has unsaved work, unless you add
   `--force`.

## Documentation

- [Command reference](docs/cli.md): every `moor` command, the global flags,
  the exit codes, and the `--json` envelope.
- [Configuration reference](docs/config.md): the user config, `.moor.yaml`,
  the controller config, and the vault key.
- [Box providers and the Proxy](docs/providers.md): providers, Box image
  requirements, and how a Box reaches its Proxy.
- [Set up the Slack front](docs/slack.md): run `moor-slack` and configure its
  Slack app.
- [Run the web front](docs/web.md): run `moor-web` on the controller host.
- [Development](docs/development.md): the crate layout, the build, and the
  checks.
