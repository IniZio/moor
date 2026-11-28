# moor (fresh)

This branch is a fresh start of moor in Go. A team gives a coding agent a
prompt on a project and gets back a result page. The spec lives outside this
repository for now.

This first commit is the scaffold only. It holds the package layout, the
domain and wire types, and the lint rules that keep the structure.

## Run the checks

```sh
make setup   # installs the pinned golangci-lint and govulncheck
make ci      # audit, lint, lint self-test, tests
```

## Structure rules

`.golangci.yml` holds every rule:

- depguard keeps each package to its allowed imports.
- revive rejects meaningless package names and files over 500 lines.
- funlen rejects functions over 60 lines.
- nolintlint makes every suppression name its linter and give a reason.
- exhaustive rejects a switch that misses a case.

`lint/fixtures` holds one violation per rule, each in its own Go module.
`make lint-selftest` fails unless the expected rule rejects each fixture.
