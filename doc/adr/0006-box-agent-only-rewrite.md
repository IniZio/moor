# Box agent only; rewrite instead of refactor

Status: Accepted, 2026-10-11

moor's agent always runs in the Box. The host engine, the agent-location setting, the artifacts Report backend and the herdr plugin are removed, and the code is rewritten in two crates (`moor-core` and `moor`) instead of refactored in place. This supersedes the "host mode stays supported" clause of 0005.

## Context

0005 moved the default agent into the Box but kept host mode. Keeping both costs the codebase its shape: agent activity is decided in about eight modules, host-versus-Box branches run through start, close and launch, and about 80 tests quietly depend on the host default. The Slack front door must start, dispatch and steer a Task with no host agent in the loop, so a design that needs a host agent is a blocker. Credentials already reach the Box agent only through the per-Task Proxy, so the reason 0004 kept the agent on the host no longer holds.

An architecture review found 13 deepening candidates. The owner judged them insufficient for the state of the code and approved a parity suite of 362 cases so a rewrite can be proven equivalent to the old behavior.

## Decision

- The Engine seam has one adapter, a Box agent kit parameterised by an agent profile. Claude is the one profile. The agent-location setting, its flag, and the host-only code paths are removed; an unknown agent-location key is a config error.
- The artifacts Report backend is dropped. Reports are local. A Report backend seam returns only after a pilot shows it is needed.
- The herdr plugin is dropped at cutover. herdr stays the one Surface for v1, optional and not deepened.
- The code lives in two crates: `moor-core` holds pure logic, `moor` holds the root, operations, adapters, store, Proxy and CLI. The Proxy stays in `moor` as a subcommand.
- Box agents get MCP servers through the Proxy, so no MCP secret enters the Box. Dotfiles run on every Task, faster than before. A steer answers a blocked prompt.
- State rows and their event are written in one transaction. The CLI returns a single JSON envelope, and `show` replaces `pane` and `reports`; `list` remains an alias of `show`.
- Old code is a reference only. The parity suite, run through an adapter for the new CLI, is the proof of equivalence. The CLI and JSON shape may change; the adapter absorbs it.
- v1 state is not migrated. This is a hard cutover with no compatibility shims.

## Considered Options

- **Keep host mode (0005 as is):** rejected. It keeps every location branch, a second source of agent activity, a second MCP delivery and a second brief, and it blocks a front door with no host agent. The one use it served, a host agent doing the work with no Box, is not a workflow moor needs.
- **Progressive refactor of the existing code:** rejected. The 13 deepening candidates leave free-string state written in seven places, 27 ad-hoc process spawns, config re-read about ten times, and domain code that prints. Each fix is a cross-cutting change that the current structure makes expensive to do one at a time, and the test suite cannot yet prove each step safe. A rewrite against the parity suite does it once.
- **Five crates with sans-IO workflows:** rejected. Two crates already enforce purity at the compiler level; an effect interpreter adds plumbing for a fifteen-step `start` and little else.
- **Event-sourced log as the source of truth:** rejected. State rows plus their event in one transaction give the same atomicity without event versioning, and a recorded process id on in-flight work detects a provisioning run that died.
- **A separate Proxy crate:** rejected for now. Revisit if the Proxy gains a heavy dependency or its own release cadence.
- **A Claude-specific Engine adapter:** rejected in favor of the agent-neutral kit. A second engine variant would not reintroduce the host-versus-Box split, because that rule is about where the agent runs, not which agent it is.

## Consequences

- Parity rows that exercise the host agent are dropped or converted to Box-agent tests.
- New capability lands Box-first and Proxy-first. Anything that needs a host agent present is out of scope.
- The glossary gains Box state, Run, Activity and Decision.
