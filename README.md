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
  run (`--for idle|done|run`). `--timeout S` bounds the wait.
- `moor steer NAME TEXT` sends text, or `--menu N` a menu choice, to the
  Task's agent.
- `moor cp`, `moor open`, `moor sleep` and `moor wake` copy a file into the
  Box, open the Workspace, and sleep or wake the Box.
- `moor gate` records or checks a gate; `moor env [REPO]` previews the
  layered agent environment.
- `moor call` runs a JSON request read from stdin.

`report`, `remove`, `shell`, `github setup`, `github login` and `proxy check`
parse but return the error "`moor <cmd>` is not implemented yet".

Global flags: `--json`, `--quiet`, `--state-dir DIR`. State lives in
`moor.db` inside the state directory, set by `--state-dir`, `MOOR_STATE_DIR`,
`XDG_STATE_HOME`, or `HOME`.

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
  plus the file-length and lint self-test scripts.

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
- `make ci`: audit, lint, format-check, then test

CI runs `format-check`, `lint`, `audit`, and `unit` in
[.github/workflows/ci.yaml](.github/workflows/ci.yaml).
