# Box port forward

A person opens a web app that runs in their Box. They use their own browser,
at `http://localhost:<port>` on their own computer. moor carries each TCP
connection from that local port to the same port on the Box's loopback.

## Why

Take a Box that runs a Docker Compose stack whose web app signs in through
OAuth, and whose only registered redirect URI is `http://localhost:5177`. The
agent cannot sign in, because the identity provider asks for a password and a
second factor. The person must, in their browser, against the Box's app.

Today nothing does this.

- A Box VM on the cloud-hypervisor runtime gets its network from passt, started
  with no inbound forwards
  ([`src/vm.rs:153`](https://github.com/IniZio/crabbox-cloudhypervisor/blob/62ac1e75062cc778c184789c31f5d7dfa6cd56ef/src/vm.rs#L153)
  in crabbox-cloudhypervisor).
- moor removed ssh to Boxes for users. `moor exec` and `exec_in_box` carry one
  command's stdio, not a listening port.
- A public URL per Box is the wrong shape. Each Box would need its own redirect
  URI registered with the provider. A forward to the person's `localhost` keeps
  the one already registered.

## Flow

```
$ moor-hc forward fix-login 5177
http://localhost:5177  →  port 5177 in fix-login. Ctrl-C ends the forward.
12:01:03  connection 1 open
12:01:40  connection 1 closed, 412 KiB in, 9 KiB out
```

1. The person asks to forward port 5177 of Box `fix-login`.
2. The client sends an `open` to the controller. The controller checks that the
   person may act on the Box, and wakes the Box if it sleeps. It checks that the
   Box sits on a node, and asks the node whether anything listens on 5177 in
   the Box. It records `port_forward_opened`.
3. The client binds `127.0.0.1:5177`, and `[::1]:5177` where the computer has
   IPv6 loopback, since a browser may resolve `localhost` to either. The local
   port is the Box port by default, because the redirect URI names it. A taken
   port fails. `--local-port N` picks another.
4. For each connection the browser makes, the client opens one `/front`
   WebSocket with a `forward` request and pipes the bytes both ways.
5. The person signs in. The provider redirects to `http://localhost:5177/...`,
   which reaches the Box's app. The bytes pass untouched, so the app sees
   `Host: localhost:5177`.
6. Ctrl-C ends the forward. The client sends a `close`, and the controller
   records `port_forward_closed` with the counts.

## Where the listener lives

The listener must run on the person's computer, so only a local front can hold
it. The code lives once in `moor-client`, as a `forward` module, and each local
front calls it.

| Surface | Holds a forward | Why |
| --- | --- | --- |
| `moor-hc forward BOX PORT` | Yes, the teammate's entry | `moor-hc` already runs on the person's computer, signed in with `moor-hc login`, and pipes bytes to a Box through `/front` for hc ([`moor-hc/src/exec.rs`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor-hc/src/exec.rs)). It runs in any terminal pane of the TUI. |
| `moor forward NAME PORT` | Yes, first and dev only | Proves the path before the bridge ships. Same client code. |
| moor-herdr | Not planned | It stays only until herdr is an hc client. It could call the same module from a pane. |
| Slack, account page, MCP | No | They run on a server. A listener there is not the person's `localhost`. Slack may print the `moor-hc` line. |

## Data path

```
browser ──tcp localhost:5177──▶ moor-hc forward              person's computer
        ──wss HOST/front, Bearer, one WebSocket per connection──▶ gate
        ──relay to the controller socket──▶ controller        forward request: authz, placement
        ──links broker at the runner the link row names──▶ machine runner
        ──yamux stream on the agent link, op port{slug, port}──▶ machine agent   node
        ──tcp 127.0.0.1:N──▶ crabbox tunnel --id slug --local-port 0 5177   ssh -L, one per slug and port
        ──ssh channel──▶ Box 127.0.0.1:5177
```

| Hop | Reuses | New |
| --- | --- | --- |
| Client to gate | `/front` WebSocket with a bearer token, binary messages for the byte stream ([`moor-gate/src/http.rs:140`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor-gate/src/http.rs#L140), [`moor-gate/src/relay.rs`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor-gate/src/relay.rs)) | Nothing in the gate |
| Gate to controller | Request frames, then bytes frames of at most 64 KiB, as `exec` sends them ([`controller/conn/exec.rs:30`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor/src/controller/conn/exec.rs#L30)) | `Request::Forward`, dispatched beside `Exec` and `Attach` ([`controller/conn.rs:130`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor/src/controller/conn.rs#L130)) |
| Controller to runner | The links broker call that `dial` makes for ssh to a pool Box ([`proxy/links.rs:133`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor/src/proxy/links.rs#L133), [`link/broker.rs:172`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor-client/src/link/broker.rs#L172)) | `PoolOp::Port` |
| Runner to agent | One yamux stream per op, a JSON line, `Reply::Started`, then raw bytes, as `Dial` does ([`link/ops.rs:148`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor-client/src/link/ops.rs#L148)) | `Op::Port`, served by pool hosts only ([`machine-agent/src/serve.rs:104`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor-machine-agent/src/serve.rs#L104)) |
| Agent to Box | The agent's crabbox home and lease, which `Dial` already resolves ([`pool/reach.rs:121`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor-machine-agent/src/pool/reach.rs#L121)) | A tunnel table in the agent |

### The stream rides the existing link

The agent link is a WebSocket with yamux, one stream per op (see
[`machine-runner-rollout.md`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/docs/specs/machine-runner-rollout.md)). `Dial` already carries raw bytes after `Started`
([`pool/reach.rs:154`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor-machine-agent/src/pool/reach.rs#L154)).
A port connection has the same shape with another far end. So there is no new
listener, port, key or Service. Forward bytes share the node's link with exec,
sync and hook streams. yamux gives each stream its own window. So one busy
stream should not stall the others. That is inferred, not measured.

### The tunnel runs on the node

The agent runs `crabbox tunnel --id <slug> --local-port 0 <port>`, reads the
`http://127.0.0.1:N` line it prints once its listener accepts, connects, answers
`Started` and splices.

- One tunnel per lease and port, started by the first connection and shared by
  the later ones.
- It stops 60 s after its last connection ends. It also stops before the agent
  pauses or stops the lease, and when the agent exits. crabbox reaps its own ssh
  process group.
- It needs only an unprivileged loopback port and the agent's own crabbox home.
  No sudo.
- `crabbox ports --publish` is not an option. It is provider-native publishing
  for docker-sandbox and codesandbox, and the cloud-hypervisor provider has none.

The tunnel's listener sits on the node's loopback while it runs. Any process on
the node can connect to it. passt maps a guest's gateway address to the host's
loopback unless started with `--no-map-gw`. The adapter passes no such flag. If
that holds on a node, another Box there can reach the listener too, and
any other loopback service on the node. This was checked on engine-03 and is
real, and adapter v0.2.2 fixes it. See the open questions.

## Who may forward

- The Box's user and the operator, by the check every Box request passes
  ([`ops/authz.rs`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor/src/ops/authz.rs)).
  Today they are the people who may write to the Box. A later rule for shared
  Boxes applies to forwards with no change, since a forward calls the same check.
- A forward reaches no more than `exec` does. A person who may exec in the Box
  can already reach any port in it. So forwards take no port list of their own.
- The gate takes only a token issued to moor's own client on `/front`
  ([`token.rs:160`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor-gate/src/token.rs#L160)),
  and sets the request's identity from it
  ([`rewrite.rs:114`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor-gate/src/rewrite.rs#L114)).
  A client cannot forward as someone else.
- No MCP tool forwards, since an agent cannot hold a listener on a person's
  computer.
  MCP tokens carry `moor:read` and `moor:write` and never reach `/front`. A later
  MCP surface for forwards must require `moor:write`.
- Nothing asks for GitHub admin, a fine-grained PAT or an installed GitHub App.
- The agent serves `port` only for leases it hosts, and trusts the runner's call
  as it does for `dial`. The controller has made the check by then.

## Lifetime

| What happens | Open connections | Local listener | Next connection | Recorded |
| --- | --- | --- | --- | --- |
| Person ends the forward | Cut | Closed | n/a | `port_forward_closed` |
| Person's computer sleeps or loses network | Cut when the WebSocket drops | Stays | Redials | Nothing |
| Box sleeps | Cut; the agent stops the lease's tunnels before the pause | Stays | Refused, asleep | `asleep`, as today |
| Box closes | Cut | The client ends the forward on the refusal | Refused, closed | `closed`, as today |
| Box gets new hands | Cut | Stays | Reaches the new placement | Nothing |
| Controller roll | Finish, or cut at the 30 s drain cap; a request in the drain gets `restarting`. The record of recent traffic that holds autosleep off is per process, so a roll forgets it | Stays | The gate holds, then dials the new controller (controller replicas spec) | Nothing |
| Runner roll | Finish on the draining runner, or cut at its 30 s budget | Stays | Routed by the link row to the new runner (machine runner rollout spec) | Nothing |
| Gate roll | Cut | Stays | Another gate replica | Nothing |
| Agent restart or update | Cut, tunnels end | Stays | New tunnel after relink | Nothing |
| Token expires | Run on | Stays | The client refreshes as `moor-hc` does; a refusal says to sign in again | Nothing |

- Opening a forward wakes an asleep Box, as an act of the person. A connection
  never wakes one. A browser tab that retries would keep the Box awake forever,
  which is why `moor-hc exec` refuses an asleep Box too.
- Bytes through a forward in the last 10 minutes count as Box activity, so
  autosleep waits. An open forward with no traffic does not hold the Box awake.
- A forward names the Box, not its hands. Each connection resolves the current
  placement, so a forward survives a rebuild.
- The agent updates itself only once no stream is open. The 30 minute idle cut
  below bounds how long a forgotten connection holds that off.

## Limits

| Limit | Value | Enforced by | Why |
| --- | --- | --- | --- |
| Ports forwarded per Box at once | 8 | Agent, tunnels per lease | Bounds ssh children on the node |
| Connections per Box at once | 64 | Agent | The cap machine forwards use ([`keeper/link.rs:140`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor/src/proxy/keeper/link.rs#L140)) |
| Connections per forward at once | 64 | Client | The same, before any stream opens |
| Idle connection | Cut after 30 minutes with no byte either way | Controller | Frees streams a browser left open. An app's WebSocket reconnects. |
| Idle tunnel | Stopped 60 s after its last connection | Agent | |
| Bytes | No cap. Counted per forward and recorded on close | Client and controller | The person's own traffic. Throughput is not measured. |

## Events

Box events, listed in [`events.md`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/docs/events.md) and left out of a
webhook's default kinds.

| Kind | Data | Written |
| --- | --- | --- |
| `port_forward_opened` | `port`, `local_port` | At open, by the controller |
| `port_forward_closed` | `port`, `connections`, `bytes_in`, `bytes_out` | At the client's close |
| `port_forward_refused` | `port`, `reason` | When an open is refused for any reason but authz |

- A client that vanished writes no `port_forward_closed`. The Box's next `asleep`
  or `closed` marks the end.
- An authz refusal writes nothing, since `not your Box` must not reveal whether a
  Box exists.
- No event per connection. The agent's recent ops already name each `port` op
  for the operator.

## Failure messages

The message goes to the forward's terminal. The browser sees its connection
closed.

| Case | Message |
| --- | --- |
| Not the person's Box, or no such Box | `not your Box` |
| Box asleep, on a connection | `fix-login is asleep; open the forward again to wake it` |
| Box closed | `fix-login is closed; the forward ended` |
| Box not on a node | `port forward needs a Box on a node; fix-login runs on <provider>` |
| Nothing listens in the Box | `nothing listens on port 5177 in fix-login` |
| Local port taken | `localhost:5177 is in use on this computer; stop what holds it or pass --local-port` |
| Node not linked | `fix-login's node <name> is not linked to moor` |
| Agent too old | `the machine agent on <name> does not forward ports yet; it updates itself while linked` |
| Port cap | `fix-login already forwards 8 ports` |
| Tunnel failed | `cannot forward port 5177 in fix-login: <crabbox's stderr tail>` |

## Decisions

| Decision | Reason |
| --- | --- |
| Tunnel on the node, by crabbox | The controller stays a byte pipe and holds no child per forward. A controller roll cuts connections, not tunnels. |
| Not the driver's controller-side `local_tunnel` | For a pool Box it runs `ssh -L` in the controller over `moor link-dial` ([`adapters/crabbox/tunnel.rs:197`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/crates/moor/src/adapters/crabbox/tunnel.rs#L197)). That puts a listener for a Box port in the controller pod, and a child that dies on each roll. |
| Not `exec` of a pipe command per connection | It works today with no core change, so it suits a one-off check of the sign-in. As the feature it costs an ssh session and an `exec_ran` event per browser connection, wakes the Box, and needs a pipe tool in the image. |
| One `/front` WebSocket per connection | Reuses the gate hold, redial and identity rewrite, and a failure stays with one connection. The cost is one handshake per browser connection, and browsers reuse connections. |
| Same port number by default | The redirect URI names the port. Picking another silently would break the sign-in. |
| Listener in `moor-client`, entry in `moor-hc` | One implementation for every local front. `moor-hc` is the surface a teammate already runs. |
| Raw TCP, no HTTP rewriting | The app must see `localhost:<port>`, as the provider redirects there. |
| A new connection never wakes an asleep Box | Only opening the forward wakes it. A browser tab that retries would otherwise keep the Box awake forever. |
| Traffic in the last 10 minutes holds autosleep off | A person using the app is using the Box. An open forward with no traffic does not. |
| No `.moor.yaml` port declaration yet | Name ports in config later, after the first live use. |
| No controller-side `local_tunnel` fallback for other providers | Nodes only in the first version, until a Box elsewhere needs a forward. |
| One WebSocket per connection until a page load is measured | Multiplexing waits for a measurement of a real page load through the forward. |
| Slack-thread sharers get no forwards | Forwards follow whatever later rule lets people write to a Box. |

## PRs, in order

1. **This spec.**
2. **Agent `port` op.** `Op::Port`, `PoolOp::Port`, the tunnel table, the 8 and
   64 caps, the listen check. The check is an agent test against a fake crabbox.
   Then, on a node, a byte round trip to a listener in a lease. About 400
   lines.
3. **Controller `forward` request.** Open, connection and close. Authz, wake,
   placement, events, idle cut and drain. The check uses a fake driver and
   agent. A connection in flight across a controller roll finishes or is cut at
   the cap. The next one succeeds. About 500 lines.
4. **Client and dev CLI.** `moor-client` forward listener and `moor forward`.
   The check is a browser on a teammate's computer. It signs in to a Compose
   app in a Box on a node, through `localhost:5177`. About 300 lines.
5. **`moor-hc forward` and docs.** The docs are [`events.md`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/docs/events.md),
   [`cli.md`](https://github.com/IniZio/moor/blob/ae08875ff9438b65e66c6623d84eaff66d817ae4/docs/cli.md) and the use guide. About 200 lines.

## Open questions

| Question | State |
| --- | --- |
| Can a guest reach the node's loopback through passt's gateway mapping? | Confirmed on engine-03: a Box reached a server bound only to the host's `127.0.0.1` through its gateway `192.168.0.1`. It is fixed in crabbox-cloudhypervisor #65, released as v0.2.2 and live on engine-03: a woken Box cannot reach the host's loopback. moor does not work around it. A node on an older adapter still exposes a forward's tunnel listener, and any other loopback service on the node, to its other Boxes. That is a known limit, not checked by the agent; `moor doctor` should say to update the adapter to v0.2.2 or later on every node that serves forwards. |

## Out of scope

- A public URL per Box, and provider-native port publishing.
- UDP.
- Forwarding into another person's Box.
- Reverse forwards, from the Box to the person's computer.
- Rewriting HTTP, such as the Host header or cookies.
- A Proxy rule that injects a secret into a request body. That is its own spec.
