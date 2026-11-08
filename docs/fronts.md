# Fronts

A front is a process that lets people reach the controller. Each one talks to
the controller over its own socket, `<state dir>/fronts/<namespace>/front`, or
over `ledger_tls` from another pod. Every flag below also reads the
environment variable shown.

## moor-gate

The HTTPS front. It checks OIDC tokens and serves `/mcp`, the account page,
browser sign-in, webhook setup, and machine pairing. It uses the `authgear`
namespace.

| Flag | Environment | Default | Meaning |
| --- | --- | --- | --- |
| `--issuer` | `MOOR_GATE_ISSUER` | required | The issuer tokens must name in `iss`. |
| `--client-id` | `MOOR_GATE_CLIENT_ID` | required | The client id tokens must name in `aud`. |
| `--front-socket` | `MOOR_GATE_FRONT_SOCKET` | none | The controller's `authgear` front socket. |
| `--controller` | `MOOR_GATE_CONTROLLER` | none | `tls://HOST:PORT`, the controller's `ledger_tls`, in place of `--front-socket`. |
| `--controller-ca`, `--controller-cert`, `--controller-key` | `MOOR_GATE_CONTROLLER_*` | none | The TLS files for `--controller`. |
| `--front-key` | `MOOR_GATE_FRONT_KEY` | none | The key the gate's hello presents to `--controller`. |
| `--listen` | `MOOR_GATE_LISTEN` | `0.0.0.0:8443` | The address. TLS is the ingress's job. |
| `--mcp-resource-url` | `MOOR_GATE_MCP_RESOURCE_URL` | none | The public `/mcp` URL. `/mcp` is off without it. |
| `--public-url` | `MOOR_GATE_PUBLIC_URL` | none | The public origin. Browser sign-in is off without it. |
| `--jwks-url` | `MOOR_GATE_JWKS_URL` | discovery | The issuer's keys. |
| `--cookie-key` | `MOOR_GATE_COOKIE_KEY` | per process | Keys of 32 or more characters that seal sign-in cookies. Comma-separated to rotate. |
| `--drain-timeout-secs` | `MOOR_GATE_DRAIN_TIMEOUT_SECS` | 30 | How long a SIGTERM waits for requests in flight. |

Over `--controller`, a request waits up to 30 s for the controller, then gets
`503` with `Retry-After: 5`. At most 512 requests wait on one gate.

## moor-slack

The Slack front, over Socket Mode. It uses the `slack` namespace.

| Flag | Environment | Meaning |
| --- | --- | --- |
| `--config PATH` | `MOOR_SLACK_CONFIG` | Required. The front's config file. |
| `--socket PATH` | `MOOR_SLACK_SOCKET`, else `MOOR_FRONT_SOCKET` | The controller's `slack` front socket. |
| `--state-dir DIR` | `MOOR_SLACK_STATE_DIR` | The front's state, including `slack.db`. Defaults to `MOOR_FRONT_STATE_DIR`, then `$XDG_STATE_HOME/moor-slack`. |
| `--controller`, `--controller-ca`, `--controller-cert`, `--controller-key`, `--front-key-file` | none | For a front in another pod, as for the gate. |

### Config keys

| Key | Default | Meaning |
| --- | --- | --- |
| `app_token_file` | required | The `xapp-` token file, mode 0600. |
| `bot_token_file` | required | The `xoxb-` token file, mode 0600. |
| `api_base` | `https://slack.com/api` | The Slack Web API base. |
| `github_api_base` | `https://api.github.com` | Where a pasted GitHub token is checked. |
| `long_turn_minutes` | 20 | Minutes one Box turn runs before the bot posts about it. |
| `coordinator.binary` | `claude` on `PATH` | The Claude Code binary. |
| `coordinator.model` | `sonnet` | The model for each coordinator turn. |
| `coordinator.context_kib` | 32 | The context budget for one turn. |
| `coordinator.max_open_boxes` | 1 | The most Boxes one thread has open at once. |
| `coordinator.idle_minutes` | 30 | Quiet minutes before a thread's host Session ends. |
| `roleplay.users` | off | For test automation only. Lets the bot post as the listed Slack users. |

### Slack commands

Every command answers only you, except `/moor repo`, which changes the
channel.

| Command | What it does |
| --- | --- |
| `/moor repo` | Lists the channel's repos and marks the default. |
| `/moor repo add OWNER/NAME` | Adds a repo to the channel. |
| `/moor repo remove NAME` | Removes a repo. |
| `/moor repo default NAME` | Makes `NAME` the channel's default repo. |
| `/moor login [claude\|github]` | Opens a form to link your Claude or GitHub token. |
| `/moor logout [claude\|github]` | Deletes that secret. `claude` by default. |
| `/moor mcp add` | Opens a form to add an MCP server of yours, with a name, an `https://` URL, and a key. Leave the key empty to sign in over OAuth. |
| `/moor mcp list` | Lists your MCP servers. |
| `/moor mcp remove NAME` | Removes a server and its key. |
| `/moor machine add` | Replies with how to pair a machine. |
| `/moor machine list` | Lists your machines. |
| `/moor machine remove NAME` | Removes a machine. |
| `/moor profile` | Shows your agent profile repo. |
| `/moor profile set URL[#REF]` | Sets your agent profile repo. |
| `/moor profile remove` | Clears your profile. |

## moor-notify

The webhook front. It claims each queued delivery, signs it, posts it, and
reports the outcome. Its controller entry needs `webhooks: true`.

| Flag | Environment | Default | Meaning |
| --- | --- | --- | --- |
| `--socket PATH` | `MOOR_FRONT_SOCKET` | set by the controller | The `notify` front socket. |
| `--poll-ms MS` | none | 1000 | How long to wait before claiming again when nothing was queued. |
| `--allow-private` | none | off | Post to loopback and private addresses too, for local development. |

## moor-listen

A client of the gate, not a front. It prints a line per Box event, as
[Events](events.md#moor-listen-lines) lists.

| Flag | Environment | Meaning |
| --- | --- | --- |
| `--url URL` | `MOOR_MCP_URL` | The gate's `/mcp` endpoint. |
| `--token-cmd CMD` | `MOOR_TOKEN_CMD` | A shell command whose stdout is the bearer token. It runs before every connect. |
| `--box NAME` | none | Watch only this Box. Repeat for several. |

`moor listen --server URL` is the same listener, signed in with `moor login`.
