# moor

moor is a per-person workflow layer that runs coding agents in Boxes. A Box
is the unit of work: a named worktree, a Workspace, and a machine from the
Driver. Agents run in Sessions on a Box.

## Commands

These commands work today:

- `moor version` prints the crate version, the git commit, and the envelope
  schema version (`1`).
- `moor start NAME` creates a Box with its worktree and Workspace and
  acquires its machine, or attaches to the open Box of that name. Flags: `--goal`,
  `--issue`, `--worktree`, `--base`, `--model`, `--no-agent`.
- `moor show [NAME]` reports all Boxes, or one Box in detail. `--events N`
  adds the last N events. `list` is an alias.
- `moor close NAME` checks for unsaved work, releases the Box's machine, and closes the
  Workspace. The worktree is kept. `--force` closes despite unsaved work.
- `moor run NAME -- CMD...` runs a command in the Box's machine. It syncs the
  host worktree first only until an agent has launched for the Box. `--sync`
  syncs only if the Box's machine has no unsaved work; otherwise, or if that cannot be
  checked, it exits 125 and does not run: save the work first or use
  `--no-sync`, which never syncs. Not both. `--record NAME` records the run.
- `moor wait NAME` waits for the Box to be idle, done, or finished with a
  run (`--for idle|done|run|input`). `input` currently returns at the same
  points as `idle` and exists so callers state their intent. `--timeout S`
  bounds the wait (default 1800).
- `moor steer NAME TEXT` sends text, or `--menu N` a menu choice, to the
  Box's agent.
- `moor cp` copies a file or directory out of the Box's machine to the host.
  `moor open`, `moor sleep` and `moor wake` open the Workspace, and sleep or
  wake the Box's machine.
- `moor gate` records or checks a gate; `moor env [REPO]` previews the
  layered agent environment.
- `moor call` runs a JSON request read from stdin.

The Slack bot token needs the `chat:write` scope for posting and updating
messages and ephemeral replies.

Global flags: `--json`, `--quiet`, `--state-dir DIR`. State lives in
`moor.db` inside the state directory, set by `--state-dir`, `MOOR_STATE_DIR`,
`XDG_STATE_HOME`, or `HOME`.

## Slack front

`moor-slack` is a separate daemon that answers Slack over Socket Mode and
talks to the controller over its socket. Its Slack app needs an app-level
token with `connections:write`, and a bot token with `app_mentions:read`,
`channels:history`, `chat:write` and `commands`. Add a `/moor` slash command
to the app. Subscribe the app to the `app_mention` and `message.channels`
bot events. Without `message.channels` the front never sees a reply in a
thread.

A user on the `users` list runs these in a channel on the `channels` list:

- `/moor repo` lists the channel's repos and marks the default.
- `/moor repo add OWNER/NAME` adds a repo. If the controller does not know
  it, the front posts one line in the channel that names the repos it knows.
- `/moor repo remove OWNER/NAME` removes a repo.
- `/moor repo default OWNER/NAME` marks one of the channel's repos as the
  default. The previous default loses its mark.

Put Claude Code 2.1.304 or newer on the controller host and name it in the
front's config:

```yaml
coordinator:
  binary: /absolute/path/to/claude
  agent: claude     # the default, and for now the only agent
  model: sonnet     # the default
  context_kib: 32   # the default
  max_open_boxes: 1 # the default: most Boxes a thread has open at once
```

Every key has a default but `binary`. A config with no `coordinator` section
gets every default and no binary. At start the front runs `claude --version`
and logs it. A missing binary or an older version leaves the coordinator
unavailable, and then no thread can start a Box.

A mention at the top level of a channel on the `channels` list opens a
thread with a coordinator agent. There is no start command: the coordinator
starts Boxes when the starter asks for work. Each message from the thread's starter becomes one turn:
the front posts a placeholder, runs one `claude -p` process with the
starter's own token, and edits the placeholder into the reply. Messages that
arrive during a turn go into the next turn together. Each turn rebuilds its
context from `slack.db`, within `context_kib`: the first request, what
earlier turns did to Boxes, the thread's Boxes, as many earlier messages and
replies as fit, and the new messages. A turn has no built-in tools, a
cleared environment and its own config dir per Slack user. It runs in
`threads/<channel>-<thread ts>` under the state directory, and refuses to
start when that directory or one of its ancestors holds a `CLAUDE.md`. A turn
that runs past 10 minutes is killed. Up to 4 turns run at once.

The coordinator's only tools come from the front. For each turn Claude Code
starts `moor-slack mcp`, named by its absolute path in the thread's
`mcp.json`, and stops it when the turn ends. The tool server reads the
thread's rows in `slack.db` and asks the controller. It never sees a Slack
or Claude token. A turn whose first output line lists any other tool or tool
server is killed. The tools act only on the thread's own Boxes:

- `list_boxes` shows each Box with its repo, who asked for it, and its state
  as the controller reports it.
- `start_box` starts a Box with a goal, on a repo of the channel, or on the
  channel's default when none is named. Only the thread's starter can ask
  for one, and a thread has at most `max_open_boxes` open Boxes. Each Box is
  a child Session of the thread's host Session, named
  `slack-<thread ts>-<n>`.
- `steer_box` sends text to a Box's agent.
- `close_box` closes a Box without force. The thread is told when it has
  closed, or why the controller refused.

A tool that changes a Box works only in a turn that holds a message from
the person who asked for that Box. Each such call is recorded in `slack.db`
before it acts, and later turns see what it did. A turn that the front
reruns after a restart cannot repeat what the cut-off turn did.

Only the thread's starter talks to the coordinator. Anyone else who writes
in the thread gets one line that says only the starter can direct it.

Each person links their own Claude token. Turn on Interactivity in the Slack
app config. Socket Mode needs no request URL. A user on the `users` list
runs these in any channel:

- `/moor login` opens a form. Run `claude setup-token` on your own machine
  and paste the token it prints. The front checks the token with one call
  to Claude on `haiku`. It stores the token only when Claude accepts it, in
  `credentials/claude/<Slack user id>` under the state directory, mode 0600.
  The token reaches Claude Code on a pipe at descriptor 3, never in argv,
  the environment, `slack.db` or the log.
- `/moor logout` deletes your token file.

Both commands take an optional agent name. `claude` is the default and, for
now, the only agent.

`moor-slack` keeps its state in `slack.db`. A `slack.db` that another build
of `moor-slack` wrote at a different schema is refused at startup. Move it
aside and start again with a fresh one.

## Box provider

moor runs Boxes through crabbox. The provider is `box.provider` in your user
config, `$XDG_CONFIG_HOME/moor/config.yaml`. A repo cannot set it. When
unset, the provider is `local-container`.

To switch from Namespace (`provider: namespace`) to an external provider,
change the provider. Then cap live Boxes for that provider with
`drivers.external.max_boxes`:

```yaml
box:
  provider: external
drivers:
  external:
    max_boxes: 5
```

Set `max_boxes` to the number of instances your provider workspace allows.
5 is an example. It takes precedence over `box.max_boxes`.

`drivers.<provider>.lookup_timeout_secs` sets how many seconds moor waits for
crabbox to look up a Box's ssh target. The default is 5, or 20 for `external`,
because an external provider's lookup goes through its adapter. Raise it if
commands fail with "box operation timed out".

`drivers.<provider>.strip_env` lists env var names that the provider's
platform puts in the Box, such as platform tokens. moor removes them from
every Box exec it runs: setup, teardown, `moor run`, and the agent (before its
hooks). The agent also starts without crabbox's own variables: its gate names,
`NSC_*` and `token`. The external provider adapter's README lists the names its
platform injects (for agent37, see crabbox-agent37 below). Every other Box
environment variable reaches the agent, so list platform secrets in `strip_env`.

```yaml
drivers:
  external:
    strip_env: [PLATFORM_TOKEN]
```

The external provider for agent37 VMs is
[crabbox-agent37](https://github.com/IniZio/crabbox-agent37). Its README lists
the crabbox user config it needs, including `external.capabilities.pauseResume:
true`. moor needs the [IniZio/crabbox](https://github.com/IniZio/crabbox)
build 0.72.0-moor.9 or a newer `-moor` build. It refuses an older build and
any upstream crabbox release, because only the `-moor` builds have the flags
moor uses.

With `pauseResume` on, moor sleeps an idle Box after `box.sleep_after`
(default `30m`). It pauses the instance, and resumes it on wake. Keep
agent37's own auto-sleep off: moor sleeps the Box itself, and a Box with a
live tunnel never looks idle to agent37. Without `pauseResume`, the Box
cannot sleep and stays up.

### Box image requirements

The Box image must provide `sh`, `git`, `curl`, `tar` and `unzip`. moor's
scripts run under `sh` and use `git` for the checkout. When the repo has
setup lines, the setup script checks for these tools first. If one is missing,
it prints a warning naming it, in the terminal and in `.moor/setup.log`, and
carries on. Setup does not fail because of it. A repo with no setup lines
runs no setup script, so nothing is checked.

The image's sshd must not set `GatewayPorts yes`. With `yes`, the Box's
Proxy port listens on the Box's network, and other machines on that network
can use the Box's Proxy. The default, `no`, keeps the port on the Box's
loopback.

### How a Box reaches its Proxy

The Box reaches its Proxy through a reverse tunnel. The tunnel listens on
`127.0.0.1:<port>` inside the Box and carries each connection to a Unix
socket in `/tmp/moor-<uid>` on the host. Only your user can open that socket.
The Proxy checks no credential from the Box, so the Box holds no Proxy
secret.

This does not hold for a Box that shares the host's network. crabbox
`local-container` with `network: host` is one. Its default, `bridge`, is not.
On such a Box, the Proxy port is on the host's loopback. Any process on the host, including
other users' processes and other Boxes, can send requests through that Box's
Proxy and get the credentials it injects. moor does not refuse such a Box.
Keep `bridge` unless you trust everything that runs on the host.

## Direct hosts

`proxy.direct` lists hosts the Box reaches without the moor Proxy, such as
public package mirrors. It is allowed in `.moor.yaml` and in your user config.
moor uses the user's entries, then any repo entries not already listed.

```yaml
proxy:
  direct: [dl.google.com, repo.maven.apache.org, .gradle.org, pub.dev]
```

An entry is a host name, or `.domain` for every name under the domain. moor
adds the entries to `NO_PROXY` and `no_proxy` as written. In the Java and
Gradle `nonProxyHosts` it writes, `.domain` becomes `*.domain`. The loopback
entries moor always sets stay first.

A credential rule's host can never go direct. moor refuses an entry that
covers one, naming the entry and the host. An entry covers its own name and
every name under it, in either form, because curl and Go read a bare
`NO_PROXY` name that way. So `.github.com` and `anthropic.com` are refused, as
is `api.github.com`. A repo's `proxy:` section holds only `direct`, so a repo
cannot add or change a rule.

## JSON envelope

`--json` prints one envelope on stdout:

```json
{
  "schema": "moor/1",
  "cmd": "version",
  "class": "ok",
  "value": { "version": "...", "commit": "...", "schema": 1 },
  "warnings": [{ "code": "...", "message": "..." }],
  "error": null
}
```

`class` is one of `ok`, `failed`, `usage`, `refused`. On failure `value` is
`null` and `error` is `{ "kind", "message", "hints", "context" }`.

## Layout

- [crates/moor-core](crates/moor-core): types and decision logic shared by
  the binary and tests: names, the `Request` enum, the `Response` envelope,
  views, and the `Secret` type.
- [crates/moor](crates/moor): the `moor` binary and library: argument
  parsing, text and JSON rendering, exit codes, the state store, and one
  module per command in [ops](crates/moor/src/ops).
- [tools](tools): [layering](tools/layering), a check that each module only
  uses the modules listed for it in [tools/layering.tsv](tools/layering.tsv)
  (`module` and the comma-separated `allowed` modules; empty means none;
  an unlisted module is unconstrained),
  plus the file-length, lint self-test, and PR overlap scripts.

## Build and test

`make setup` installs the pinned tools. The Rust toolchain is pinned in
[rust-toolchain.toml](rust-toolchain.toml).

- `make format-check`: `cargo fmt --check`, `taplo fmt --check`
- `make lint`: clippy with `-D warnings`, the 400-line file cap, the layering
  check, `typos`, and `cargo doc` with warnings denied
- `make audit`: `cargo deny check`, `cargo machete`
- `make unit`: unit tests, the CLI test, and doc tests through `cargo nextest`
  and `cargo test`
- `make test`: the same as `make unit`
- `make overlap BASE=<ref> [HEAD=<ref>] [FILES="glob ..."]`: lists open PRs
  that touch the same files and are not yet in `BASE`. Run it before
  branching new work and before rebasing a stack, then stack on what it lists.
- `make ci`: audit, lint, format-check, then test

CI runs `format-check`, `lint`, `audit`, and `unit` in
[.github/workflows/ci.yaml](.github/workflows/ci.yaml).
