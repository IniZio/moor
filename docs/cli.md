# Command reference

The `moor` CLI is a dev tool. It sends one request to moor's core and prints
the result. `moor help <command>` prints the full flag list of a command.

## Where a command runs

A command runs on a controller when one is configured. moor picks the first
of these that is set:

1. The `MOOR_CONTROLLER` environment variable, which holds a socket path.
2. `controller.ssh` in the user config. moor runs
   `ssh -T <destination> moor controller attach`.
3. `controller.socket` in the user config.

With none of them set, the command runs in its own process against the local
state directory.

When the client of a `moor run` goes away, the controller stops streaming the
command and records the run as interrupted. The command in the Box gets no
hangup, so it can keep running until it next writes output. Ctrl-C does this on
purpose: moor asks the controller to stop the run, waits for its answer, and
exits 130. A second Ctrl-C exits 130 at once.

## Global flags

| Flag | Effect |
| --- | --- |
| `--json` | Print one JSON envelope on stdout. |
| `--quiet` | Suppress warnings in text output. |
| `--state-dir DIR` | Use `DIR` as the state directory. |

The state directory holds `moor.db`. It is the first of `--state-dir`,
`$MOOR_STATE_DIR`, `$XDG_STATE_HOME/moor`, and `$HOME/.local/state/moor`.

## Box commands

| Command | What it does |
| --- | --- |
| `moor start NAME` | Creates a Box with its worktree, herdr workspace, and machine, or attaches to the open Box of that name. The agent starts when you give `--goal` or `--resume`. The Box acts for the moor user of whoever started it, so `start` runs only through a controller, and refuses an open Box of another user. It also refuses when the user lacks a secret the Box needs, naming every missing one. See [Secrets a Box uses](config.md#secrets-a-box-uses). |
| `moor show [NAME]` | Shows all open Boxes, or one Box in detail. `list` is an alias. |
| `moor run NAME -- CMD...` | Runs a command on the Box's machine. |
| `moor wait NAME` | Waits until the Box reaches a state. |
| `moor wait-events NAME...` | Waits for events from one or more Boxes. |
| `moor steer NAME TEXT` | Sends text to the Box's agent. `--menu N` sends a menu choice instead. |
| `moor end NAME` | Ends the agent Session and keeps the Box open. |
| `moor cp NAME SRC DEST` | Copies a file or directory from the Box's machine to the host. |
| `moor open NAME` | Wakes the Box if it sleeps, then focuses its herdr workspace. |
| `moor sleep NAME` | Puts the Box's machine to sleep. |
| `moor wake NAME` | Wakes a sleeping Box. |
| `moor close NAME` | Closes the Box and releases its machine. |

### `moor start` flags

| Flag | Effect |
| --- | --- |
| `--goal TEXT` | The agent's first prompt. |
| `--issue REF` | An issue reference for the Box. |
| `--base REF` | The ref the Box branch starts from. |
| `--repo NAME\|URL` | A repo from the controller config, cloned on the controller host, instead of the current directory. |
| `--model MODEL` | The agent model. |
| `--no-agent` | Do not start an agent. |
| `--hold` | Start the agent with no first prompt. |
| `--resume` | Resume the agent's stored session. |
| `--trust-moor-yaml` | Start the agent even when the Box's `.moor.yaml` differs from its base commit. |
| `--parent ID` | The parent Session. The default is `$MOOR_SESSION`. |

### `moor show` flags

| Flag | Effect |
| --- | --- |
| `--events N` | Add the last `N` events. |
| `--agent` | Add the agent state. |
| `--reports` | Add the Box's reports. |
| `--pane` | Add the end of the agent's terminal pane. |
| `--session ID` | Show one Session and its child Sessions instead of a Box. |

### `moor run` flags

The command runs under bash when the Box has bash, else under sh. One `CMD`
argument is shell text, so `moor run NAME -- 'make lint | tee log'` works.

`moor run` runs the command in the Box checkout as it is. moor copies the host
worktree into a Box only when it provisions the Box's machine.

| Flag | Effect |
| --- | --- |
| `--record NAME` | Record the run as the gate `NAME`, for `moor gate check`. moor reads the Box checkout's HEAD first and records the run under it. It refuses a Box checkout with uncommitted changes. If the command moves HEAD or changes a tracked file, the run fails with "the Box HEAD moved during the record run". New untracked files do not fail it. |
| `--trust-moor-yaml` | Run even when the Box's `.moor.yaml` differs from its base commit. |

### `moor wait` flags

| Flag | Effect |
| --- | --- |
| `--for STATE` | `idle` (the default), `done`, `input`, or `run`. `idle`, `done`, and `input` all return when the agent is not working and no run is in progress. `run` returns when no run is in progress. |
| `--timeout S` | Give up after `S` seconds. The default is 1800. |

### `moor wait-events` flags

| Flag | Effect |
| --- | --- |
| `--kind KIND` | Match only this event kind. You can repeat it. |
| `--all` | Wait for every Box instead of any one. |
| `--since N` | Only events after event id `N`. |
| `--timeout S` | Give up after `S` seconds. |

### `moor close` flags

| Flag | Effect |
| --- | --- |
| `--force` | Close even when the Box has unsaved work. You lose that work. |
| `--trust-moor-yaml` | Run the teardown from the worktree's `.moor.yaml` when it differs from the base commit. Without this flag, the teardown uses the base commit's copy and prints a warning. |

`moor close` removes the worktree that moor created only when the worktree's
branch is synced from the remote and the worktree has no unpushed or
uncommitted work. Otherwise moor keeps the worktree and prints a warning.

### `moor cp` paths

`SRC` is a path on the Box. moor resolves a relative `SRC` from the Box's
workdir. A relative `SRC` must not contain `..`. `DEST` is a path on the host, relative to the
current directory.

## Session commands

| Command | What it does |
| --- | --- |
| `moor session open NAME --seed TEXT` | Starts a new agent Session on an open Box. `--seed-file PATH` reads the first prompt from a file. |
| `moor session open --host --workdir DIR` | Opens a host Session in `DIR`, with no Box. `--agent NAME` and `--agent-session-id ID` set its agent. Host Sessions are the operator's only. |
| `moor session set ID` | Changes the `--agent` or `--agent-session-id` recorded on a host Session. |
| `moor session end ID` | Ends a host Session. |

`moor session open` takes `--parent ID`, which defaults to `$MOOR_SESSION`.

## User commands

These commands act as your `local:<uid>` identity through a running
controller.

| Command | What it does |
| --- | --- |
| `moor user enroll` | Creates your user, or shows the one you have. |
| `moor user secret set NAME` | Stores secret `NAME`. moor reads the value from stdin, never from the arguments. |
| `moor user secret rm NAME` | Deletes secret `NAME`. |
| `moor user secret ls` | Lists the names of your secrets. |
| `moor user link-code` | Prints a code that links another identity to your user. The code works once, within 10 minutes. |
| `moor user link CODE` | Links your identity to the user that printed `CODE`. |
| `moor whoami` | Shows your identity, its user, and that user's linked identities. |

## Other commands

| Command | What it does |
| --- | --- |
| `moor version` | Prints the version, the git commit, and the envelope schema version. |
| `moor env [REPO]` | Previews the settings and credentials a Box in `REPO` would get. The default is the current repo. Only the operator may run it. |
| `moor gate check SHA NAME` | Exits 0 when a run recorded with `moor run --record NAME` passed on commit `SHA`, and 1 when none did. `--command TEXT` counts only runs of exactly that command text. It reads only the runs of Boxes that you may act on. It does not check that `SHA` exists on origin. |
| `moor events` | Prints stored events as JSON lines. Takes `--since ID`, `--box NAME`, `--kind KIND`, and `--follow`. |
| `moor call` | Reads one JSON request from stdin and runs it. The output is always a JSON envelope. |
| `moor controller serve --config FILE` | Runs the controller. See [the controller config](config.md#controller-config). |
| `moor controller attach` | Connects stdin and stdout to the controller socket. `--socket PATH` overrides the socket. |

A `moor call` request names the command in `cmd`, in kebab case:

```sh
echo '{"cmd":"show"}' | moor call
```

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | The command succeeded. |
| 1 | The command failed. |
| 2 | The command line or request was not valid. |
| 3 | moor refused the command. |
| 124 | `moor wait` timed out. |
| 125 | `moor run` did not start the command. |
| 130 | The command that `moor run` started was interrupted. |

For any other result, `moor run` exits with the code of the command it ran.

## JSON envelope

With `--json`, every command prints one envelope on stdout:

```json
{
  "schema": "moor/1",
  "cmd": "version",
  "class": "ok",
  "value": { "version": "0.2.0", "commit": "...", "schema": 1 },
  "warnings": [{ "code": "...", "message": "..." }],
  "error": null
}
```

| Field | Value |
| --- | --- |
| `schema` | Always `moor/1`. |
| `cmd` | The command that ran. |
| `class` | `ok`, `failed`, `usage`, or `refused`. |
| `value` | The command's result. `null` on failure. |
| `warnings` | A list of `{ "code", "message" }`. |
| `error` | `null` on success. Otherwise `{ "kind", "message", "hints", "context" }`, where `kind` is `usage`, `refused`, `failed`, `internal`, or `not_found`. |
