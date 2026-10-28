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
hangup, so it can keep running until it next writes output. On Ctrl-C, moor
asks the controller to stop the run, waits for its answer, and exits 130. A
second Ctrl-C exits 130 at once.

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
| `moor start NAME` | Creates a Box on the branch `moor/NAME` with its herdr workspace and machine, or attaches to the open Box of that name. The agent starts unless you give `--no-agent`. Without `--prompt` or `--issue` it waits for your first prompt, or resumes its stored session on a Box that has one. The Box acts for the moor user of whoever started it, so `start` runs only through a controller, and refuses an open Box of another user. It also refuses when the user lacks a secret the Box needs, naming every missing one. See [Secrets a Box uses](config.md#secrets-a-box-uses). |
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
| `--prompt TEXT` | The agent's first prompt. Without it the agent starts waiting for a prompt. |
| `--issue REF` | An issue reference for the Box. |
| `--base REF` | The ref on GitHub the Box branch starts from. The default is the repo's default branch. |
| `--repo OWNER/NAME\|URL` | The GitHub repo the Box clones, as `OWNER/NAME` or a github.com URL, instead of the current checkout's `origin`. |
| `--model MODEL` | The agent model. |
| `--no-agent` | Do not start an agent. |
| `--resume` | Resume the agent's stored session. |
| `--parent ID` | The parent Session. The default is `$MOOR_SESSION`. |

Without `--repo`, the CLI reads `git remote get-url origin` in its own
directory and sends that repo as `OWNER/NAME`. It refuses an `origin` that is
not a github.com repo. Outside a checkout it sends no repo, and the Box starts
with an empty workdir. The controller never reads a directory on its host for
a start.

Before any machine exists, the controller reads these from the GitHub API
with the requester's own `github` secret:

- the commit that `--base` names, or the default branch's commit
- `.moor.yaml`, `.mcp.json`, and `.claude/settings.json` at that commit
- the name of the token's account

moor keeps that `.moor.yaml` in the Box's row, and every later command uses
that copy. The controller refuses a repo that the account cannot read. It also
refuses a Box whose branch `moor/NAME` already exists on GitHub, unless
`--base moor/NAME` names it. An account that cannot push gets a warning.
Commits in the Box use the account's profile name, or its login, and its
GitHub noreply address.

The start works from what is on GitHub. Uncommitted or unpushed changes in
your checkout are not in the Box, and the CLI warns about them. A branch with
no upstream counts as unpushed, because without `--base` the Box starts from
the default branch.

The Box clones its repo itself, through its Proxy, from
`https://github.com/OWNER/NAME.git`. The clone is partial
(`--filter=blob:none`). It has the full history and fetches file contents when
a command first needs them. A retried setup keeps the Box branch where it is,
so it never resets commits made in the Box. moor does not support submodules
or Git LFS objects from other private repos, because the Proxy adds the user's
token only for the Box's own repo.

Work leaves the Box when the agent runs `git push origin HEAD:refs/heads/BRANCH`.
To get it into your own checkout, run `git fetch origin BRANCH` there.

### `moor show` flags

| Flag | Effect |
| --- | --- |
| `--events N` | Add the last `N` events. |
| `--agent` | Add the agent state. |
| `--reports` | Add the Box's reports. |
| `--pane` | Add the end of the agent's terminal pane. |
| `--session ID` | Show one Session and its child Sessions instead of a Box. |

### What `moor show` reports about the machine

While a Box starts, one command inside it reads how many CPUs it can run on
(`nproc`) and its total memory (`MemTotal` in `/proc/meminfo`). moor runs that
command in the same exec as the clone, so it adds no call to the Box.
`moor show NAME --json` and the reply of a start put the result under
`driver`. The text form prints a `machine:` line after the `box:` line.

| Field | Meaning |
| --- | --- |
| `driver.cpus` | CPUs the Box can run on, as `nproc` counts them. |
| `driver.mem_mib` | Total memory in MiB. The kernel keeps some of the memory a provider gives, so the value is a little under the size asked for. |

Both fields are `null` in these cases:

- A start has not read them yet.
- The Box printed nothing usable.
- The Box has no repo and no commit author, so it runs no clone exec.

A Box started again on a new machine records that machine's numbers.

### `moor run` flags

The command runs under bash when the Box has bash, else under sh. One `CMD`
argument is shell text, so `moor run NAME -- 'make lint | tee log'` works.

`moor run` runs the command in the Box checkout as it is. moor never copies
a host checkout into a Box.

| Flag | Effect |
| --- | --- |
| `--record NAME` | Record the run as the gate `NAME`, for `moor gate check`. moor reads the Box checkout's HEAD first and records the run under it. It refuses a Box checkout with uncommitted changes. If the command moves HEAD or changes a tracked file, the run fails with "the Box HEAD moved during the record run". New untracked files do not fail it. |

### `moor wait` flags

| Flag | Effect |
| --- | --- |
| `--for STATE` | `idle`, `done`, `input`, or `run`. The default is `idle`. `idle`, `done`, and `input` all return when the agent is not working and no run is in progress. `run` returns when no run is in progress. |
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
| `--force` | Close even when the Box has unsaved work. Unsaved work is commits that are on no branch on GitHub, and uncommitted files. You lose that work. Without `--force`, `moor close` refuses and says how to push the work with `moor run NAME -- git push origin HEAD:refs/heads/BRANCH`. |

moor creates no host worktree for a Box, so `moor close` checks only the
Box checkout for unsaved work.

### `moor cp` paths

`SRC` is a path on the Box. moor resolves a relative `SRC` from the Box's
workdir. A relative `SRC` must not contain `..`. `DEST` is a path on the host,
relative to the current directory.

When `DEST` is a directory, the copy goes inside it. Only you can read a copied
file (mode 0600) or a copied directory (mode 0700). A copied file replaces a
file already at the destination. Nothing replaces a directory there, and a
copied directory replaces nothing, not even an empty directory. moor refuses a
`SRC` that is or holds a symlink or a special file. It also refuses a `DEST`
that is a symlink. A failed copy leaves nothing at the destination. If moor is
killed during a copy, it can leave a `.moor-cp-*` directory beside the
destination. The next copy there removes any such directory older than an
hour.

## Session commands

| Command | What it does |
| --- | --- |
| `moor session open NAME --seed TEXT` | Starts a new agent Session on an open Box. `--seed-file PATH` reads the first prompt from a file. |
| `moor session open --host --workdir DIR` | Opens a host Session in `DIR`, with no Box. `--agent NAME` and `--agent-session-id ID` set its agent. A host Session belongs to the user who opens it. |
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
| `moor whoami` | Shows your identity, its user, and that user's linked identities. |

## Other commands

| Command | What it does |
| --- | --- |
| `moor version` | Prints the version, the git commit, and the envelope schema version. |
| `moor env [OWNER/NAME] [--base REF]` | Previews the settings and credentials a Box of that GitHub repo would get, read through GitHub like a start. The default is the current checkout's `origin`. Only the operator may run it. |
| `moor gate check SHA NAME` | Exits 0 when a run recorded with `moor run --record NAME` passed on commit `SHA`, and 1 when none did. `--command TEXT` counts only runs of exactly that command text. It reads only the runs of Boxes that you may act on. It does not check that `SHA` exists on origin. |
| `moor events` | Prints stored events as JSON lines. Takes `--since ID`, `--box NAME`, `--kind KIND`, `--parent SESSION` (only events of that Session's child Sessions), and `--follow`. |
| `moor call` | Reads one JSON request from stdin and runs it. The output is always a JSON envelope. |
| `moor controller serve --config FILE` | Runs the controller. See [the controller config](config.md#controller-config). |
| `moor controller attach` | Connects stdin and stdout to the controller socket. `--socket PATH` overrides the socket. |

A `moor call` request names the command in `cmd`, in kebab case:

```sh
echo '{"cmd":"show"}' | moor call
```

### `box-mirror`

`{"cmd":"box-mirror","id":ID,"dest":DIR,"exclude":[PATTERN,...]}` makes the host directory `DIR`
an exact copy of the work tree of the open Box with store id `ID`. It has no
CLI command. A client sends it through `moor call` or moor-client. Only the
operator may send it. `DIR` must be absolute, and moor creates it when it is
missing.

moor runs rsync over the Box's ssh connection. The mirror keeps links as links
and keeps file modes. It deletes from `DIR` the files that are gone from the
Box, and it leaves out `.git`. A later mirror sends only what changed. The
result gives `files`, the regular files sent, and `bytes`, the bytes that came
from the Box. moor refuses a Box without a ready machine. It also refuses when
rsync is missing on the Box or on the host, and the error names which one.

### `box-put`

`{"cmd":"box-put","id":ID,"src":FILE,"dest":PATH}` writes the host file `FILE` into the work
tree of the open Box with store id `ID`, at `PATH`, with mode 0644. It has no CLI command. A
client sends it through `moor call` or moor-client. Only the operator may send it.

`FILE` must be an absolute path to a regular file of at most 32 MiB. `PATH` is relative to
the work tree. moor refuses an empty path, an absolute path, and a path with `..`. It
creates missing directories and replaces an existing file. The result gives `path`, the
file's place in the Box, and `bytes`.

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
