# Development

## Crate layout

| Crate | What it holds |
| --- | --- |
| [moor-core](../crates/moor-core) | Types and decision logic with no IO: names, config parsing, the `Request` enum, the `Response` envelope, views, and the `Secret` type. |
| [moor](../crates/moor) | The `moor` binary and library: argument parsing, text and JSON output, exit codes, the state store, the controller, the Proxy, the adapters for crabbox, herdr, and git, and one module per command in [ops](../crates/moor/src/ops). |
| [moor-migrate](../crates/moor-migrate) | Forward-only schema migrations for the `SQLite` databases, run by [refinery](https://github.com/rust-db/refinery). |
| [moor-client](../crates/moor-client) | A client for the controller socket that fronts use. |
| [moor-slack](../crates/moor-slack) | The Slack front. |
| [moor-listen](../crates/moor-listen) | Holds the gate's listen stream open and prints a line per Box event. |
| [moor-notify](../crates/moor-notify) | The webhook front: posts a person's Box events to a URL. |
| [moor-web](../crates/moor-web) | The web front. |
| [moor-testkit](../crates/moor-testkit) | Helpers that the other crates' tests share. |

[tools](../tools) holds the lint scripts that `make lint` runs. One of them,
[layering](../tools/layering), checks that each module uses only the modules
that [layering.tsv](../tools/layering.tsv) lists for it. Each row is a
`module` and its comma-separated `allowed` modules. An empty list allows
none, and a module with no row has no limit. A row `extern:<crate>` confines
an outside crate to the modules it lists: to use it elsewhere, keep that code
in an allowed module and give the caller a trait, as `moor::wire` does. The
check skips `#[cfg(test)]` items, `tests.rs` files and `tests/` directories,
but not a file such as `status_tests.rs`.

[migration-numbers](../tools/migration-numbers) compares the store's
migration list with the base branch's, `origin/develop` unless you pass
`BASE=<ref>`. A step this branch adds must have a version above the base's
highest, and a version the base already has must keep its name. Two parallel
branches that both add the next number fail here, before the second merges,
because renumbering an applied migration breaks the stores that ran it. In
CI the base is the pull request's base branch.

Each failing check prints a `fix:` line. In short:

- `removed-names`: a retired name came back. Use what replaced it. The old
  name of the removed work-unit concept is banned in every file; a name moor
  must read from another tool, such as a hook's JSON field, goes in
  [removed-names.allow](../tools/removed-names.allow) as that exact string.
- `box-key`: a name derived from a Box id was built by hand. Build it with a
  `BoxKey` method in
  [box_key.rs](../crates/moor-core/src/box_key.rs).
- `layering`: a module used one its row does not list. Either the row should
  allow it, or the code belongs elsewhere.
- `test-children`: test code spawned a process and held its `Child` outside
  `moor_testkit::ChildGuard`, so a panicking test would leave it running.
  Wrap it with `ChildGuard::new(..)`, or use `.output()` or `.status()`. A
  file that cannot goes in [test-children.allow](../tools/test-children.allow)
  with the reason. The check reads `tests/` directories, `tests.rs` files and
  `#[cfg(test)]` and `#[test]` items.

## Database migrations

Each database's schema is an ordered list of migrations. moor.db's are written
once in Rust with sea-query, in [crates/moor-schema](../crates/moor-schema),
and rendered for SQLite and for Postgres. moor-slack and moor-web still use
`V<n>__<name>.sql` files. When moor opens a database, it applies the missing
migrations in one `BEGIN IMMEDIATE` transaction and records each one in
`refinery_schema_history`. A second process that opens the database waits for
the whole run.

moor.db's first migration, `V45__baseline`, replaced the SQL migrations 1 to
44 with the schema they built. moor adopts a database recorded through
migration 44 as already at 45, and refuses one recorded through less, or one
with no migration history that already has tables. Start such a state dir
afresh. moor refuses a database that holds a migration this build does not
know, so an older build never reads a newer file.

A schema change is a new migration. Do not edit an applied migration. A test
pins the checksum of every migration as each backend renders it, and refinery
refuses a database whose applied migration changed. Keep changes additive, as the engineering
handbook's guide to non-breaking changes asks. Add tables and nullable or
defaulted columns, and ship a data patch separately from the schema change.
Migrations run before the new code uses them, and a deploy never moves a
database back.

## Developing moor inside a moor Box

Running a Box from inside a Box needs `/dev/kvm`, because the inner Box is a
cloud-hypervisor VM. The node's VM provides it as `root:kvm` with mode `0660`,
so the Box user must be in the `kvm` group. The repo's `.moor.yaml` sizes the
Box `large` and runs `tools/box-nested-kvm.sh`, which adds the user to `kvm` and
installs pinned `cloud-hypervisor`, `passt`, `crabbox` and
`crabbox-cloudhypervisor` into the `nested-kvm` cache volume. A Box on a host
without `/dev/kvm` skips the script. A Box that runs as root skips the group
change, because root opens `/dev/kvm` without it and the image may have no
`kvm` group. A new group applies to new logins, so
start a command that opens `/dev/kvm` in a Box session that already exists with
`sg kvm -c '<command>'`.

`make nested-e2e` checks the whole path. It builds moor with the pty surface,
unless `target/debug/moor` is newer than every tracked file, runs a controller
with one `runtime: external` node that drives `crabbox-cloudhypervisor`
(2 vCPU, 4 GiB), and starts a Box on `IniZio/moor-nested-check`, a scratch repo
with no `.moor.yaml`. Start the outer Box with `--also IniZio/moor-nested-check`
so its Proxy holds credentials for that repo, or set `NESTED_E2E_REPO` to another
scratch repo you can push to.
That Box writes `NESTED.md`, pushes its `moor/*` branch and opens a draft PR.
The script records `systemd-detect-virt` from inside the VM, then closes the
Box, the PR and the branch. On exit, success, failure or Ctrl-C, it stops every
process it started and removes the VM disks. Its logs are in
`.moor/dev/nested-e2e/out`. GitHub and Claude credentials come from the outer
Box's Proxy through `HTTPS_PROXY`, so the inner secrets are placeholders.

## Build and test

`make setup` installs the pinned tools as prebuilt binaries with `cargo-binstall`, and builds one from source only when its release has no binary. [rust-toolchain.toml](../rust-toolchain.toml) pins the Rust toolchain.

In a Box, keep `~/.cargo/bin` between Boxes with the `cargo-bin` [cache volume](config.md#cache-volumes), so `make setup` runs once. A crabbox config without the `cache-volume` feature cannot keep that volume, so every Box runs `make setup` again.

| Target | What it runs |
| --- | --- |
| `make build` | `cargo build --workspace --release --locked`. |
| `make lint` | `cargo fmt --check` and `taplo fmt --check`, Clippy with `-D warnings`, the 400-line cap on Rust files, the removed-name, Box-key, and Box-environment checks, the layering, test-children, and migration-number checks, the lint self-tests, `typos`, and `cargo doc` with warnings denied. |
| `make prepush` | `make audit`, then `make lint`. These are the two gates that CI runs before any test. |
| `make hooks` | Opts in to a pre-push hook. It sets `core.hooksPath` to [tools/git-hooks](../tools/git-hooks), so `git push` first runs `make prepush`. A push that deletes a branch skips it. Push with `MOOR_SKIP_HOOKS=1` to skip the hook. Run `git config --unset core.hooksPath` to opt out. |
| `make audit` | `cargo deny check` and `cargo machete`. |
| `make unit` | Every test target of the workspace in one `cargo nextest` run, with the doc tests alongside it. |
| `make test` | The same as `make unit`. |
| `make ci` | `audit`, `lint`, then `test`. |
| `make overlap BASE=<ref> [HEAD=<ref>] [FILES="glob ..."]` | Lists open pull requests that touch the same files and are not yet in `BASE`. |

Run `make overlap` before you branch new work and before you rebase a stack.
Base the new branch on the pull requests that it lists.

CI runs `lint`, `audit`, and `unit` in
[ci.yaml](../.github/workflows/ci.yaml).
On a pull request, the full run happens only on the tip of a stack, the PR that no open PR is based on.
Its green run covers every PR below it.
If you close the PR above, push to the one below to start its run.

## Run a Box from a checkout

This starts one Box from a git repo on your machine with a CLI you built.
You need:

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
