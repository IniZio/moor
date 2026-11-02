# Get told by a line on stdout

`moor-listen` holds an MCP `subscriptions/listen` stream open on the moor gate
and prints one line to stdout per Box event that needs a person. Run it as a
background monitor, so each line wakes whatever reads its output. It knows
nothing about that reader.

```sh
moor login --server https://moor.example.com   # once
moor listen --server https://moor.example.com
```

`moor listen` uses the shared sign-in `moor login` saves, and refreshes it when
it runs out or the gate refuses it. Install the full `moor` binary; there is
nothing else to set up.

| Flag | Environment | Meaning |
| --- | --- | --- |
| `--server` | `MOOR_SERVER` | The server's origin, or its `/mcp` address. |

The `moor-listen` binary is the same listener for a token that does not come
from `moor login`:

```sh
moor-listen --url https://moor.example.com/mcp --token-cmd 'cat ~/.config/moor/token'
```

| Flag | Environment | Meaning |
| --- | --- | --- |
| `--url` | `MOOR_MCP_URL` | The gate's `/mcp` endpoint. |
| `--token-cmd` | `MOOR_TOKEN_CMD` | Shell command (`sh -c`) whose trimmed stdout is the bearer token. It runs before every connect, and its output is never logged. |

## Output

One line per event, flushed at once:

| Line | When |
| --- | --- |
| `moor: box <name> <event>: <agent_said>` | The Box's newest `idled`, `needs_input`, `agent_stalled`, `agent_resumed`, `setup_failed`, `session_failed`, `launch_timed_out`, `box_failed`, `relaunch_capped` or `closed` event. `agent_said` is the first line of what the agent said, cut at 200 characters, and the `: ...` is left out when there is none. Each event prints once. The `closed` line does not say why: the event's `data` has `from` and, when moor closed the Box on its own, `reason` (`setup_failed_expired` or `close_failed`). |
| `moor: box <name> stalled: no progress for <N> min[ while <doing>]` | The agent reported busy and has sent no report for at least 10 minutes. `while ...` names what it was doing, when it said. Printed once per stall. |
| `moor: box <name> resumed after <N> min` | A stalled agent reported again. |
| `moor-listen: stream lost (<reason>); reconnecting` | The stream ended, failed or sent nothing for 45 s, and the first reconnect failed too. A drop that reconnects at once prints nothing. Printed once per outage; it retries after 2 s, doubling to 60 s. |
| `moor-listen: reconnected` | The stream is back after a `stream lost` line. |
| `moor-listen: not signed in to <origin>; run moor login --server <origin>` | `moor listen` has no sign-in to the server. Printed once; it looks again every 60 s, so a login made later is picked up without a restart. |
| `moor-listen: token rejected; call any moor tool so the client refreshes it` | The gate answered 401 or 403 even to a freshly refreshed token. It tries again every 60 s. |

Events from before the first connect are not printed. After every reconnect
it reads each Box again and prints the events it missed while no stream was
open, so a drop loses nothing. `busy` and the other
events are skipped. stderr is for debugging.

## Example

```text
$ moor listen --server https://moor.example.com
moor: box app/fix-login idled: Opened the PR.
moor: box app/docs needs_input: Which branch should I base this on?
moor-listen: stream lost (stream ended); reconnecting
moor-listen: reconnected
```

It follows [Get told instead of polling](mcp.md#get-told-instead-of-polling):
it lists the Boxes, listens on each one, reads a Box when it changes, and lists
again when a Box starts or goes.
