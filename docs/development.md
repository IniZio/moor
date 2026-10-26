# Development

## Crate layout

| Crate | What it holds |
| --- | --- |
| [moor-core](../crates/moor-core) | Types and decision logic with no IO: names, config parsing, the `Request` enum, the `Response` envelope, views, and the `Secret` type. |
| [moor](../crates/moor) | The `moor` binary and library: argument parsing, text and JSON output, exit codes, the state store, the controller, the Proxy, the adapters for crabbox, herdr, and git, and one module per command in [ops](../crates/moor/src/ops). |
| [moor-migrate](../crates/moor-migrate) | Forward-only schema migrations for the `SQLite` databases, run by [refinery](https://github.com/rust-db/refinery). |
| [moor-client](../crates/moor-client) | A client for the controller socket that fronts use. |
| [moor-slack](../crates/moor-slack) | The Slack front. |
| [moor-web](../crates/moor-web) | The web front. |
| [moor-testkit](../crates/moor-testkit) | Helpers that the other crates' tests share. |

[tools](../tools) holds the lint scripts that `make lint` runs. One of them,
[layering](../tools/layering), checks that each module uses only the modules
that [layering.tsv](../tools/layering.tsv) lists for it. Each row is a
`module` and its comma-separated `allowed` modules. An empty list allows
none, and a module with no row has no limit.

## Database migrations

Each database's schema is an ordered list of `V<n>__<name>.sql` files. The
files for moor.db are in [crates/moor/migrations](../crates/moor/migrations).
When moor opens a database, it applies the missing files in one
`BEGIN IMMEDIATE` transaction and records each one in
`refinery_schema_history`. A second process that opens the database waits for
the whole run.

moor adopts a database that has no migration history as already at
migration 1 when its old schema version is the last one shipped without
migrations. It refuses any other database without history. Migration 2 of
moor.db creates the tables that older builds added on every open, if they are
missing, so an adopted file that lacks one of them gets it. moor refuses a
database that holds a migration this build does not know, so an older build
never reads a newer file.

A schema change is a new migration file. Do not edit an applied migration. A
test pins the checksum of every file, and refinery refuses a database whose
applied migration changed. Keep changes additive, as the engineering
handbook's guide to non-breaking changes asks. Add tables and nullable or
defaulted columns, and ship a data patch separately from the schema change.
Migrations run before the new code uses them, and a deploy never moves a
database back.

## Build and test

`make setup` installs the pinned tools. [rust-toolchain.toml](../rust-toolchain.toml) pins the Rust toolchain.

| Target | What it runs |
| --- | --- |
| `make build` | `cargo build --workspace --release --locked`. |
| `make lint` | `cargo fmt --check` and `taplo fmt --check`, Clippy with `-D warnings`, the 400-line cap on Rust files, the removed-name, Box-key, and Box-environment checks, the layering check, the lint self-tests, `typos`, and `cargo doc` with warnings denied. |
| `make audit` | `cargo deny check` and `cargo machete`. |
| `make unit` | Every test target of the workspace in one `cargo nextest` run, with the doc tests alongside it. |
| `make test` | The same as `make unit`. |
| `make ci` | `audit`, `lint`, then `test`. |
| `make overlap BASE=<ref> [HEAD=<ref>] [FILES="glob ..."]` | Lists open pull requests that touch the same files and are not yet in `BASE`. |

Run `make overlap` before you branch new work and before you rebase a stack.
Base the new branch on the pull requests that it lists.

CI runs `lint`, `audit`, and `unit` in
[ci.yaml](../.github/workflows/ci.yaml).
