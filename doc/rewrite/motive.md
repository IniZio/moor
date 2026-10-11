# Rewrite motive

## Objective

Rewrite moor so the agent always runs in the Box, and prove the new code matches the old behavior before cutover. The old code spreads host-versus-Box branches across many modules, and tests quietly depend on the host default. Moor's Slack front door must start and steer a Task with no host agent. The rewrite must keep the same capabilities and state outcomes. The command-line surface and JSON shape may change. The cutover is hard, with no compatibility shims.

## Decisions

### Rewrite, with a parity suite as proof

We chose a rewrite on a fresh branch, not progressive refactoring, because the architecture review found 13 deepening candidates and the owner judged them too little for the state of the code. We chose "same capabilities and state outcomes", not exact command-line and JSON parity, so the interface can be redesigned. The suite drives only the moor command line and fake external tools. It runs through a client adapter, so a redesign changes only the adapter. We chose a standalone test crate on the new branch, not raw command-line calls in tests. Each outside tool gets one small recording fake, not the old bash fakes. Tests use a test-only clock, not real-time sleeps. The old moor is frozen except for critical fixes.

### Box agent only

We chose to always run the agent in the Box, not to keep host mode. The agent-location setting, its flag and the host engine are removed. Host mode keeps every location branch and a second way to deliver MCP servers and briefs. It also blocks a front door that has no host agent. Parity rows that need a host agent are dropped or turned into Box-agent tests. This goes further than ADR 0005, which kept host mode.

### Box agent features

We chose to give Box agents MCP servers through the Proxy, not to remove MCP. Stdio MCP servers that need a secret stay rejected for now, not bridged in this rewrite. We chose to install dotfiles on every Task and make that faster, not to skip them after the first launch. We chose that a steer answers a blocked agent's prompt, not that it is refused. Box credentials reach only host-driven Box commands, not the agent pane. The agent gets GitHub and Claude access only through the Proxy.

### Reports

We chose local Reports only, not the artifacts backend. The backend returns only after a pilot shows it is needed.

### Shape of the code

We chose two crates, a pure core and the moor crate, not five crates with sans-IO workflows. Two crates already enforce purity at compile time. We chose state rows plus their event in one transaction, not an event-sourced log. It gives the same atomicity without event versioning. We chose the Proxy as `moor proxy serve` in the same binary, not a separate crate. Seams get one implementation each, named by role: Forge, not GitHub; an agent-neutral Box agent kit with Claude as the one profile, not a Claude-named adapter. Proxy routes are rows of data, not hard-coded. We chose not to add a second adapter or profile now. A second agent kind would not bring back the host split, because that rule is about where the agent runs.

### Command line

We chose one JSON envelope and a `show` command that replaces `pane` and `reports`. We chose hard removal of `pane` and `reports`, not aliases. `list` stays as an alias of `show`. `moor run` exits 125 when moor fails before the command runs, not exit 1, because exit 1 is ambiguous with a command that exits 1. Config errors stay exit 1, not exit 2, because a bad key lives in a file and different flags never fix it. An unknown config key is a parse error that names the key path, not a warning.

### Behavior changes

We chose these new behaviors over the old ones:
- `close` refuses while the Task branch has unpushed commits, unless `--force`. We did not keep the old no-gate behavior.
- A failed repo `setup:` leaves the Task working and records the failure. It does not stop the Task.
- `start` re-runs setup when the last setup failed, not only attach.
- Git name and email apply to Task commits in the worktree and in the Box, not only for display.
- The transcript is saved on agent exit and on close, with no duplicates, not on close only.
- `close` may remove the caller's own Workspace without the detached-session rule.

### Proxy

We chose no path or method allow-list on the model route, not a limit to `/v1/messages` and the auth endpoints. A list would break when Claude Code updates. No TLS interception.

### Surface

We chose to keep herdr for v1 as an optional Surface, not to deepen it or switch to Slack now. Slack is the preferred eventual surface. Headless agent sessions are a backlog item, so steering later needs no terminal multiplexer. The herdr plugin is dropped at cutover, not kept. The herdr adapter uses a dedicated session named `moor`, not the user's default session, which would disturb their layout. We chose not to run tmux in the Box, because it adds a dependency. We chose not to rely only on herdr's experimental handoff for updates. The adapter parses herdr JSON tolerantly and maps "needs restart" to an unavailable error with a hint. After a restart, moor resumes the Claude session by its stored id, not with a fresh brief.

### Tooling

We chose strict lints and a 400-line cap per production file, not default-only lints or a looser cap. A layering check runs in CI. We chose `make ci` before every commit, not git hooks. This lands right after the parity suite, so it does not pause the suite.

## Open questions

- Can crabbox give an SSH target for reading a Box file without taking its lease lock? If not, reading a Box file needs a patch to the crabbox fork. Answer before the Box file read is built.
- How do we start the named herdr session's server headless? Herdr documents the server command and the session variable, but not the two together. Answer before the herdr adapter is built.
- Does herdr accept a registered resume command whose first word is `moor` on the user's PATH? If not, resume after a herdr restart falls back to the relaunch rule.
