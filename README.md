# moor

moor runs coding agents in Boxes. A Box is one piece of work. It has a
branch of a GitHub repo, a herdr workspace that shows the agent's terminal,
and a machine from crabbox where the agent runs and clones the repo. The agent does
not get your Claude or GitHub token. A Proxy on your host adds each token to
the agent's requests.

moor is for engineers who run several agents at once and want each one on
its own machine and branch. People reach moor through fronts, such as the
Slack front. The `moor` CLI is a dev tool to drive and debug the core. It is
not the main user interface.

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
- crabbox 0.79.1-moor.6 or a newer `-moor` build of
  [IniZio/crabbox](https://github.com/IniZio/crabbox) on `PATH`, set up for
  the `local-container` provider
- a GitHub token, for a repo on GitHub, such as the one `gh auth token` prints
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

2. Run a controller, and point the CLI at it. A Box acts for the moor user
   of whoever starts it, so `moor start` runs only through a controller. Keep
   the controller config outside the state directory, because the controller
   keeps its vault key beside the config:

   ```sh
   mkdir -p ~/.config/moor-controller
   printf 'socket: ~/.local/state/moor/controller.sock\n' > ~/.config/moor-controller/config.yaml
   moor controller serve --config ~/.config/moor-controller/config.yaml &
   export MOOR_CONTROLLER=~/.local/state/moor/controller.sock
   ```

3. Create your moor user, and store your tokens as its secrets. The CLI acts
   as your `local:<uid>` identity, and each command reads the value from
   stdin. You need the `github` secret only for a repo on GitHub:

   ```sh
   moor user enroll
   printf '%s' '<the Claude token>' | moor user secret set claude
   gh auth token | moor user secret set github
   ```

   A Box's Proxy adds these secrets to the agent's requests. Nothing comes
   from your shell or your user config.

4. Name a node for new Boxes in your user config. moor has no default
   node, and `moor start` refuses until `nodes` is set:

   ```sh
   mkdir -p ~/.config/moor
   printf 'nodes:\n  local:\n    runtime: local-container\n' > ~/.config/moor/config.yaml
   ```

5. In your repo, preview the settings a Box would get:

   ```sh
   cd ~/src/your-repo
   moor env
   ```

   Each line names a setting and the layer it came from, such as
   `box provider  [user]  local-container`.

6. Start a Box with a prompt for the agent:

   ```sh
   moor start fix-typo --prompt "Fix the typo in the README"
   ```

   Run it inside a checkout of a GitHub repo, or name one with
   `--repo OWNER/NAME`. moor creates the herdr workspace and the machine, the
   machine clones the repo, and then the agent starts.

7. Follow the Box, and close it when the agent is done:

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
- [Nodes](docs/nodes.md): where Boxes run, how the scheduler picks a node, and
  how to name one or require labels.
- [Box providers and the Proxy](docs/providers.md): providers, Box image
  requirements, and how a Box reaches its Proxy.
- [Set up the Slack front](docs/slack.md): run `moor-slack` and configure its
  Slack app.
- [Run the web front](docs/web.md): run `moor-web` on the controller host.
- [Development](docs/development.md): the crate layout, the build, and the
  checks.
