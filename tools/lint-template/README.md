Apply: paste `lints.toml` into `crates/moor/Cargo.toml` (it holds the `[lints.rust]` and `[lints.clippy]` tables).
`print_stdout`/`print_stderr` are allowed only in the CLI render module, via `#[allow(..., reason = "...")]`.
The strict set is self-tested by `tools/lint-selftest.sh` (run from `make lint`).
