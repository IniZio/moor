# moor

moor is a per-person workflow layer that runs coding agents in Boxes. It
tracks each unit of work as a Task: a named worktree, a Workspace, and a Box
the agent runs in.

## Commands

These commands work today:

- `moor version` prints the crate version, the git commit, and the envelope
  schema version (`1`).
- `moor start NAME` creates a Task with its worktree and Workspace and
  acquires its Box, or attaches to the open Task of that name. Flags: `--goal`,
  `--issue`, `--worktree`, `--base`, `--model`, `--no-agent`.
- `moor show [NAME]` reports all Tasks, or one Task in detail. `--events N`
  adds the last N events. `list` is an alias.
- `moor close NAME` checks for unsaved work, releases the Box, and closes the
  Workspace. The worktree is kept. `--force` closes despite unsaved work.
- `moor run NAME -- CMD...` runs a command in the Task's Box. It syncs the
  host worktree first only until an agent has launched for the Task. `--sync`
  syncs only if the Box has no unsaved work; otherwise, or if that cannot be
  checked, it exits 125 and does not run: save the work first or use
  `--no-sync`, which never syncs. Not both. `--record NAME` records the run.
- `moor wait NAME` waits for the Task to be idle, done, or finished with a
  run (`--for idle|done|run|input`). `input` currently returns at the same
  points as `idle` and exists so callers state their intent. `--timeout S`
  bounds the wait (default 1800).
- `moor steer NAME TEXT` sends text, or `--menu N` a menu choice, to the
  Task's agent.
- `moor cp` copies a file or directory out of the Task's Box to the host.
  `moor open`, `moor sleep` and `moor wake` open the Workspace, and sleep or
  wake the Box.
- `moor gate` records or checks a gate; `moor env [REPO]` previews the
  layered agent environment.
- `moor call` runs a JSON request read from stdin.

`report`, `remove`, `shell`, `github setup`, `github login` and `proxy check`
parse but return the error "`moor <cmd>` is not implemented yet".

The Slack bot token needs the `chat:write` scope for posting and updating
messages and ephemeral replies, and `files:write` for uploading proof files
into a Task's thread.

Global flags: `--json`, `--quiet`, `--state-dir DIR`. State lives in
`moor.db` inside the state directory, set by `--state-dir`, `MOOR_STATE_DIR`,
`XDG_STATE_HOME`, or `HOME`.

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
hooks). The sandbox never passes them through. The external provider adapter's README lists the names its
platform injects (for agent37, see crabbox-agent37 below).

```yaml
drivers:
  external:
    strip_env: [PLATFORM_TOKEN]
```

The external provider for agent37 VMs is
[crabbox-agent37](https://github.com/IniZio/crabbox-agent37). Its README lists
the crabbox user config it needs, including `external.capabilities.pauseResume:
true`. moor needs crabbox 0.72.0-moor.7 or newer, and asks you to upgrade
if yours is older.

With `pauseResume` on, moor sleeps an idle Box after `box.sleep_after`
(default `30m`). It pauses the instance, and resumes it on wake. Keep
agent37's own auto-sleep off: moor sleeps the Box itself, and a Box with a
live tunnel never looks idle to agent37. Without `pauseResume`, the Box
cannot sleep and stays up.

A repo can pick the agent37 VM image and disk in its `.moor.yaml`, next to `size`:

```yaml
box: { size: large, template: agent37-ubuntu, disk_gb: 40 }
```

moor passes `template` and `disk_gb` to the agent37 adapter on each Box
acquire in the environment variables `CRABBOX_AGENT37_TEMPLATE` and
`CRABBOX_AGENT37_DISK_GB`. Unset keys send no variable, so the provider
default applies. moor does not check the disk size; the adapter does, and
moor shows its error as is. Only the external (agent37) provider takes these
keys. With any other provider, moor refuses the acquire and names the keys.

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
