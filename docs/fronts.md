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
| `--box-listen` | `MOOR_GATE_BOX_LISTEN` | `127.0.0.1:8444` | The Box-facing listener, which serves only `report_step`. Bind loopback beside a Proxy on the host, or the pod address on a cluster. Nothing but a Proxy may reach it. `off` turns it off. A non-loopback address is refused without `--box-listen-off-host`. |
| `--box-listen-off-host` | `MOOR_GATE_BOX_LISTEN_OFF_HOST` | off | Allows `--box-listen` to bind a non-loopback address. Set it only where nothing but a Proxy can reach that address. |
| none | `MOOR_SERVICE_KEY` | unset | A shared secret for the Box-facing listener, for a cluster where a NetworkPolicy cannot keep other callers off it. When set, the Proxy stamps it as `X-Moor-Service-Key`. The listener refuses with `401` any request whose copy differs or is missing, and compares in constant time. Set but empty, the gate does not start. Unset, the listener is as before. |
| `--mcp-resource-url` | `MOOR_GATE_MCP_RESOURCE_URL` | none | The public `/mcp` URL. `/mcp` is off without it. |
| `--public-url` | `MOOR_GATE_PUBLIC_URL` | none | The public origin. Browser sign-in is off without it. |
| `--jwks-url` | `MOOR_GATE_JWKS_URL` | discovery | The issuer's keys. |
| `--cookie-key` | `MOOR_GATE_COOKIE_KEY` | per process | Keys of 32 or more characters that seal sign-in cookies. Comma-separated to rotate. |
| `--drain-timeout-secs` | `MOOR_GATE_DRAIN_TIMEOUT_SECS` | 30 | How long a SIGTERM waits for requests in flight. |

The Box-facing listener serves one MCP server, `moor`, with one tool,
`report_step`. The Proxy routes `moor.moor.internal` to it and stamps
`X-Moor-Box`, which the listener trusts as the Box's identity. The gate's entry
in the controller config needs `step_reports: true`, and the user config's
`moor_service: {upstream: <listener>}` routes the host and gives every Box the
server.

Over `--controller`, a request waits up to 30 s for the controller, then gets
`503` with `Retry-After: 5`. At most 512 requests wait on one gate.

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
