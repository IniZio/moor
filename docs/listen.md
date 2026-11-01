# Get told by a line on stdout

`moor-listen` holds an MCP `subscriptions/listen` stream open on the moor gate
and prints one line to stdout per Box event that needs a person. Run it as a
background monitor, so each line wakes whatever reads its output. It knows
nothing about that reader.

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
| `moor: box <name> <event>: <agent_said>` | The Box's newest `idled`, `needs_input`, `setup_failed`, `session_failed`, `launch_timed_out`, `box_failed`, `relaunch_capped` or `closed` event. `agent_said` is the first line of what the agent said, cut at 200 characters, and the `: ...` is left out when there is none. Each event prints once. |
| `moor-listen: stream lost (<reason>); reconnecting` | The stream ended, failed or sent nothing for 45 s, and the first reconnect failed too. A drop that reconnects at once prints nothing. Printed once per outage; it retries after 2 s, doubling to 60 s. |
| `moor-listen: reconnected` | The stream is back after a `stream lost` line. |
| `moor-listen: token rejected; call any moor tool so the client refreshes it` | The gate answered 401 or 403. It reruns the token command every 60 s. |

Events from before the first connect are not printed. After every reconnect
it reads each Box again and prints the events it missed while no stream was
open, so a drop loses nothing. `busy` and the other
events are skipped. stderr is for debugging.

## Example

```text
$ moor-listen --url https://moor.example.com/mcp --token-cmd ./token.sh
moor: box app/fix-login idled: Opened the PR.
moor: box app/docs needs_input: Which branch should I base this on?
moor-listen: stream lost (stream ended); reconnecting
moor-listen: reconnected
```

It follows [Get told instead of polling](mcp.md#get-told-instead-of-polling):
it lists the Boxes, listens on each one, reads a Box when it changes, and lists
again when a Box starts or goes.
