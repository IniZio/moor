# CLI

`moor` is the operator's and contributor's tool. `moor-machine-agent` runs on
each paired machine. `moor help COMMAND` and `moor-machine-agent help COMMAND`
print every flag.

## moor

### Where a command runs

A command goes to a controller when one is configured. moor picks the first of
these that is set:

1. `MOOR_CONTROLLER`, a socket path.
2. `controller.ssh` in the user config. moor runs
   `ssh -T DESTINATION moor controller attach`.
3. `controller.socket` in the user config. `moor up` sets it.

With none set, the command runs in its own process on the local state.

The state directory is the first of `--state-dir`, `MOOR_STATE_DIR`,
`$XDG_STATE_HOME/moor`, and `~/.local/state/moor`.

### Global flags

| Flag | Effect |
| --- | --- |
| `--json` | Print one JSON envelope on stdout. |
| `--quiet` | Leave warnings out of text output. |
| `--state-dir DIR` | Use `DIR` as the state directory. |

### Boxes

| Command | What it does |
| --- | --- |
| `moor start NAME` | Creates a Box on the branch `moor/ID-NAME`, or attaches to the open Box of that name. |
| `moor show [NAME]` | Shows all open Boxes, or one in detail. `list` is an alias. |
| `moor inspect NAME` | Shows the agent's state, why it looks stuck, its latest messages, and the end of its terminal. |
| `moor run NAME CMD...` | Runs a command in the Box's work tree and records the run. |
| `moor exec NAME CMD...` | Runs a command with stdin, stdout, and stderr passed byte for byte, and exits with its code. Records no run. |
| `moor steer NAME [TEXT]` | Sends text, a menu choice, or an interrupt to the agent. |
| `moor wait NAME` | Waits until the Box reaches a state. |
| `moor wait-events BOXES...` | Waits for events from one or more Boxes. |
| `moor cp NAME SRC DEST` | Copies a file or directory out of the Box to the host. |
| `moor end NAME` | Ends the agent Session and keeps the Box open. |
| `moor sleep NAME`, `moor wake NAME` | Puts the Box to sleep, or wakes it. |
| `moor retry NAME` | Runs the setup of a Box in `setup_failed` again. |
| `moor close NAME` | Closes the Box and releases its VM. |
| `moor close-failed` | Closes every Box of yours in `setup_failed`. |

`moor start` flags:

| Flag | Effect |
| --- | --- |
| `--repo OWNER/NAME` | The repo to clone, as `OWNER/NAME` or a github.com URL. The default is the current checkout's `origin`. |
| `--also OWNER/NAME` | Another repo to clone beside the first. Repeat for more. Replaces `box.also`. |
| `--prompt TEXT` | The agent's first prompt. Without it, the agent waits. |
| `--issue REF` | An issue reference for the Box. |
| `--base REF` | The ref to branch from. The default is the repo's default branch. |
| `--node NAME` | A node of yours, or a shared one, in place of your default. |
| `--model MODEL` | The agent model. |
| `--cold` | Set up from scratch, past any saved setup. |
| `--no-agent` | Start no agent. |
| `--resume` | Resume the agent's stored session. |
| `--parent ID` | The parent Session. The default is `MOOR_SESSION`. |

A start reads the repo from GitHub with the requester's `github` secret. Changes
that are not pushed are not in the Box, and the CLI warns about them. The start
refuses when the user lacks a secret the Box needs, and names each one.

Other flags:

| Command | Flag | Effect |
| --- | --- | --- |
| `show` | `--agent`, `--events N`, `--reports`, `--pane` | Add the agent state, the last `N` events, the Reports, or the end of the terminal. |
| `show` | `--session ID` | Show one Session and its child Sessions. |
| `inspect` | `--lines N`, `--said N` | Terminal lines, 40 by default and 200 at most. Agent messages, 5 by default and 20 at most. |
| `run` | `--record NAME` | Record the run as the gate `NAME` for `moor gate check`. |
| `exec` | `-i` | Forward stdin from a terminal too. |
| `steer` | `--menu N`, `--interrupt` | Send a menu choice, or the agent's interrupt key. |
| `wait` | `--for STATE`, `--timeout S` | `idle`, `done`, `input`, or `run`. 1800 seconds by default. |
| `wait-events` | `--kind KIND`, `--all`, `--since N`, `--timeout S` | Match kinds, wait for every Box, start after an event id. |
| `close`, `close-failed` | `--force` | Close despite commits that are not pushed or files that are not committed. That work is lost. |

### Sessions

| Command | What it does |
| --- | --- |
| `moor session open NAME --seed TEXT` | Starts a new agent Session on an open Box. `--seed-file PATH` reads the prompt from a file. |
| `moor session open --host --workdir DIR` | Opens a host Session with no Box. `--agent` and `--agent-session-id` set its agent. |
| `moor session set ID` | Changes the agent or agent session recorded on a host Session. |
| `moor session end ID` | Ends a host Session. |

### Your user

These act as your local identity through the controller.

| Command | What it does |
| --- | --- |
| `moor user enroll` | Creates your user, or shows it. |
| `moor user secret set NAME` | Stores a secret, read from stdin. |
| `moor user secret ls`, `rm NAME` | Lists secret names, or deletes one. |
| `moor user mcp add NAME URL` | Adds an MCP server of yours. `--header` names the header for its key. |
| `moor user mcp finish URL`, `ls`, `rm NAME` | Finishes an OAuth sign-in, lists servers, or removes one. |
| `moor user machine ls`, `status NAME`, `rm NAME` | Lists your machines, asks one for its state, or removes one. |
| `moor user profile set URL[#REF]`, `show`, `rm` | Sets, shows, or clears your agent profile repo. |
| `moor user webhook add URL` | Adds a webhook. `--event KIND` and `--header NAME` narrow it. |
| `moor user webhook setup ID`, `finish ID`, `ls`, `rm ID` | Shows the setup with its key, finishes it, lists, or removes. |
| `moor whoami` | Shows your identity, its user, and the user's linked identities. |
| `moor node ls` | Lists the shared nodes and yours, with their load. |

### The host

| Command | What it does |
| --- | --- |
| `moor up [--max-boxes N]` | Sets the host up, as [Self-host on one machine](run.md) describes. A rerun changes nothing. |
| `moor down` | Stops and removes the controller's user units. Config, state, and secrets stay. |
| `moor doctor [--public-url URL]` | Checks the setup, one `ok`, `warn`, or `fail` line per check, each with its fix. Exits 1 when a check failed. |
| `moor env [OWNER/NAME] [--base REF]` | Previews the settings and credentials a Box of that repo would get. Operator only. |
| `moor controller serve --config FILE` | Runs the controller. |
| `moor controller attach [--socket PATH]` | Connects stdin and stdout to the controller socket, for use over ssh. |
| `moor store migrate` | Brings the store to this release's schema and exits. |
| `moor store copy --to URL` | Copies the SQLite store into an empty Postgres database and exits. |
| `moor events` | Prints stored events as JSON lines. `--since`, `--box`, `--kind`, `--parent`, and `--follow` filter them. |
| `moor gate check SHA NAME` | Exits 0 when a run recorded as `NAME` passed on commit `SHA`. |
| `moor call` | Runs one JSON request read from stdin. |
| `moor version` | Prints the version, the commit, and the envelope schema. |

### A local client of a remote moor

These sign in to a moor server over OIDC and keep one sign-in per server for
every local tool. `--server` takes the origin or the `/mcp` address and falls
back to `MOOR_SERVER`.

| Command | What it does |
| --- | --- |
| `moor login --server URL` | Signs in in the browser. `--no-browser` prints the address and reads the pasted return. |
| `moor logout --server URL` | Forgets the sign-in and revokes it. |
| `moor status [--server URL]` | Shows who is signed in and how long the token has left. |
| `moor token --server URL` | Prints a fresh access token. `--headers` prints it as a headers helper expects. |
| `moor mcp --server URL` | Relays MCP over stdio to the server, with a fresh token on each call. |
| `moor listen --server URL [--box NAME]` | Prints one line per Box event that needs a person. See [Events](events.md#moor-listen-lines). |

### Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success. |
| 1 | The command failed, or a start or retry kept the Box after a failed setup line. |
| 2 | The command line or request was not valid. |
| 3 | moor refused the command. |
| 124 | `moor wait` timed out. |
| 125 | `moor run` did not start the command. |
| 130 | The command `moor run` started was interrupted. |

`moor run` and `moor exec` otherwise exit with the command's own code.
`moor exec` exits 255 for its own failure.

### JSON envelope

With `--json`, every command prints one envelope:

```json
{"schema": "moor/1", "cmd": "version", "class": "ok", "value": {}, "warnings": [], "error": null}
```

`class` is `ok`, `failed`, `usage`, or `refused`. `error` is `null` on
success, else `{kind, message, hints, context}`.

## moor-machine-agent

| Command | What it does |
| --- | --- |
| `up --server URL` | Pairs the machine, checks its provider, and installs a user service that runs the agent. Rerun it to apply new flags. |
| `down` | Stops and removes the user service. The machine stays paired. |
| `run` | Keeps the machine linked to its server. The user service runs it. |
| `update --version VERSION` | Installs a release in place, after checking its checksum and signature. |
| `lease VERB [SLUG]` | Runs a lease verb on this host and prints JSON. For debugging. |
| `dial SLUG` | Carries stdin and stdout to a lease's sshd, as an ssh `ProxyCommand`. |

`up` flags:

| Flag | Default | Effect |
| --- | --- | --- |
| `--server URL` | `MOOR_SERVER` | The moor server. |
| `--name NAME` | the host name | The machine's name. |
| `--labels a,b` | none | Labels a Box can ask for. |
| `--runtime PROVIDER` | crabbox's provider | Offer Boxes through this crabbox provider. |
| `--max-boxes N` | 4 | The most Boxes held at once. |
| `--allow-exec` | off | Host no Boxes. Run the `exec` commands Boxes send. |
| `--crabbox PATH` | `crabbox` | The crabbox program. |

`run` also takes these flags:

| Flag | Effect |
| --- | --- |
| `--reserve-mib MIB` | Memory kept for the host. 4096 plus a tenth of the host's memory by default. |
| `--cache-gb GB` | Disk that caches may use. |
| `--auto-update` | Install the release the server names, then restart. |
