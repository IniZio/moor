# moor (fresh)

This branch is a fresh start of moor in Go. A team gives a coding agent a
prompt on a repository and gets back a result page. The spec lives outside this
repository for now.

This first commit is the scaffold only. It holds the package layout, the
domain and wire types, and the lint rules that keep the structure.

## Run the checks

```sh
make setup   # installs the pinned golangci-lint and govulncheck
make ci      # audit, lint, lint self-test, tests
```

## Portal pages

Pages are `a-h/templ` templates in `internal/web`. `make generate` rewrites the generated Go files, and
`make ci` fails when one is stale.

The editor, htmx and the fonts are committed under `internal/web/static` and embedded in the binary, so
nothing loads from outside at run time. `make assets` rebuilds them with esbuild, from the lock file in
`assets`. It needs Node and `npm`. `make ci` does not.

Run the portal with `PORTAL_DATABASE_URL`, `PORTAL_SECRET_KEY` (base64 of 32 bytes) and `PORTAL_USER_HEADER`.
The last one names the request header that the sign-in proxy sets to the person.
A request without that header gets 401.

## Structure rules

`.golangci.yml` holds every rule:

- depguard keeps each package to its allowed imports.
- revive rejects meaningless package names and files over 500 lines.
- funlen rejects functions over 60 lines.
- nolintlint makes every suppression name its linter and give a reason.
- exhaustive rejects a switch that misses a case.

`lint/fixtures` holds one violation per rule, each in its own Go module.
`make lint-selftest` fails unless the expected rule rejects each fixture.
