# moor

moor is a per-person workflow layer that runs coding agents in Boxes. This
branch holds a walking skeleton: the crate layout, the command-line surface,
the JSON envelope, and one working command, `moor version`. Every other verb
parses but returns an "is not implemented yet" error.

## Layout

- [crates/moor-core](crates/moor-core): pure types shared by the binary and tests: names, the
  `Request` enum, the `Response` envelope, and the `Secret` type.
- [crates/moor](crates/moor): the `moor` binary and library: clap argument parsing, text
  and JSON rendering, exit codes, and the operation dispatch.
- [tools/layering](tools/layering): a check that each module only uses the
  modules listed for it in [tools/layering.tsv](tools/layering.tsv).

## Commands

`moor version` prints the crate version, the git commit, and the envelope
schema version (`1`).

Global flags: `--json`, `--quiet`, `--state-dir DIR`.

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
`null` and `error` is `{ "kind", "message", "hints", "context" }`, where
`kind` is `usage`, `refused`, `failed`, or `internal`.

## Exit codes

| Class | Code |
|---|---|
| ok | 0 |
| failed (including `internal`) | 1 |
| usage | 2 |
| refused | 3 |

For `moor run`, an ok response exits with the command's own `exit` value, and
any non-ok class exits 125 (moor failed before the command ran).

## Development

`make ci` runs, in order:

- `make audit`: `cargo deny check`, `cargo machete`
- `make lint`: clippy with `-D warnings`, the 400-line file cap, the layering
  check and its tests, the lint self-test, `typos`, and `cargo doc` with
  warnings denied
- `make format-check`: `cargo fmt --check`, `taplo fmt --check`
- `make test`: unit tests and the CLI test through `cargo nextest`

`make setup` installs the pinned tools.
