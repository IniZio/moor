# Deploy the controller with systemd

On a host, run the controller as a systemd user service with socket units.
systemd holds the listening sockets, so a client that connects while the
controller restarts waits in the socket's backlog and gets its answer from
the new controller. Each socket keeps its inode across restarts.

A development run needs none of this. Without passed sockets, `moor
controller serve` binds its own sockets.

## Install the units

Get the binaries with `install.sh` from the latest release (`sh install.sh moor moor-gate`). The units below assume they are in `~/.local/bin`, the controller
config is `~/.config/moor/controller.yaml`, and the state directory is the
default `~/.local/state/moor`. In a user unit, `%S` is that state base,
`$XDG_STATE_HOME` or `~/.local/state`. Set `XDG_STATE_HOME` to the same value
in your login shell and in the systemd user manager, or leave it unset in
both. Otherwise the CLI dials a socket path that the units never bound.

1. Write `~/.config/systemd/user/moor.socket`:

   ```ini
   [Unit]
   Description=moor controller socket

   [Socket]
   ListenStream=%S/moor/controller.sock
   FileDescriptorName=controller
   Service=moor.service
   SocketMode=0600
   DirectoryMode=0700

   [Install]
   WantedBy=sockets.target
   ```

2. If the controller config has `fronts`, write
   `~/.config/systemd/user/moor-front@.socket`. Each instance holds one
   front's socket:

   ```ini
   [Unit]
   Description=moor socket for the %i front

   [Socket]
   ListenStream=%S/moor/fronts/%i/front
   FileDescriptorName=front-%i
   Service=moor.service
   SocketMode=0600
   DirectoryMode=0700

   [Install]
   WantedBy=sockets.target
   ```

3. Write `~/.config/systemd/user/moor.service`:

   ```ini
   [Unit]
   Description=moor controller
   Requires=moor.socket
   After=moor.socket

   [Service]
   ExecStart=%h/.local/bin/moor controller serve --config %h/.config/moor/controller.yaml
   KillMode=process
   TimeoutStopSec=660
   Restart=on-failure
   ```

4. Enable the controller socket and one front socket per entry under `fronts`:

   ```sh
   systemctl --user daemon-reload
   systemctl --user enable --now moor.socket moor-front@slack.socket
   systemctl --user start moor.service
   ```

`KillMode=process` makes systemd signal only the controller on stop. The
shared egress Proxy and other processes that the controller started keep
running. systemd logs them as left-over processes when the service starts
again. `TimeoutStopSec` must be above the config's `drain_timeout_secs`,
600 by default, or systemd kills the controller in the middle of its drain.

Every Box's credential hosts go through one shared egress Proxy. Without
`MOOR_EGRESS_URL`, the controller runs it itself as `moor proxy serve-egress`
on `<state dir>/egress/egress.sock`, with the key it keeps in
`<state dir>/egress.key`. It outlives the controller, and a controller that
finds it answering keeps it. Stop it by hand to run a new build of it. Each
Box reaches it over a tunnel the controller holds, so a controller restart cuts
the transfers open at that moment, and new requests wait for the tunnel to come
back. With `MOOR_EGRESS_URL` set to a `wss://` address, Boxes dial that Proxy
themselves, and `MOOR_EGRESS_KEY` must be the key it was given.

At start the controller answers requests at once and opens each open Box's
tunnel and machine forwards in the background, so a slow Box never holds up
the API.

## What the controller checks

The controller takes the passed sockets by the `sd_listen_fds` protocol. It
uses `LISTEN_FDS` and `LISTEN_FDNAMES` only when `LISTEN_PID` is its own pid.
Otherwise it ignores them and binds its own sockets, as a development run
does.

The controller refuses to start, with exit code 1, when the passed sockets do
not match the config:

- A passed socket has a name other than `controller`, `runner`, `ledger` or
  `front-<namespace>` for a namespace in `fronts`.
- The config needs a socket that systemd did not pass. Enable the missing
  `moor-front@<namespace>.socket`.
- A socket is bound at a path other than the one the config gives. The controller
  socket path is `socket` in the config, else `<state dir>/controller.sock`.
- A socket file is open to the group or to other users. Set `SocketMode=0600`.

It also exits 1 when another controller holds its lock. A start waits up to 5 s
for the lock first, since a store migration holds it briefly. The message names
the holder's pid and socket.

The machine runner's socket, `<state dir>/runner/runner.sock`, may be passed
as `runner`. When it is not passed, the controller binds it itself.

The ledger socket, `<state dir>/ledger/proxy.sock`, may be passed as `ledger`.
When it is not passed, the controller binds it itself. The shared egress Proxy
and the front Proxy make their store calls on it, so they never hold the database
URL or the vault key. Each presents its key.

The machine runner binds `<state dir>/runner/links.sock` itself, mode 0600, and
refuses to start when the file is open to the group or to other users. The
controller and `moor link-dial` use it to reach a personal node's machine over that host's
link. It accepts only lease and dial requests.

The controller never deletes or rebinds a socket that systemd passed in.
The passed fds are close-on-exec, so Box processes and fronts do not inherit
them.

## Upgrade the controller

1. Install the new binaries over the old ones, for example with
   `install -m 755 moor moor-slack moor-web ~/.local/bin/`.
2. Restart the service:

   ```sh
   systemctl --user restart moor.service
   ```

Do not restart the socket units. That closes the sockets, and every client
that is connected or waiting loses its connection.

On the restart, systemd sends SIGTERM and the controller drains, as
[Stopping the controller](config.md#stopping-the-controller) describes:

1. The controller stops accepting connections. New connects wait in the
   backlog that the socket unit holds.
2. A `wait`, `wait-events`, or `watch` gets an error frame of kind
   `restarting` and is closed.
3. Any other request in flight, such as a `start`, runs to its end, for up to
   `drain_timeout_secs`. A settle pass that is already running also runs to
   its end. The controller stops a settle pass that outlives
   `drain_timeout_secs`, without rollback.
4. Every connection ends with an error frame of kind `restarting`, which
   answers any request the controller did not run.
5. The controller exits 0. systemd starts the new controller on the same
   sockets, and it answers the connects that waited in the backlog.

`moor-gate` drains on SIGTERM or SIGINT as well. It stops accepting, lets
requests in flight end for up to `--drain-timeout-secs`
(`MOOR_GATE_DRAIN_TIMEOUT_SECS`, 600 by default), closes open `/front` and
`/events` streams after 5 s so clients reconnect, and exits 0.

While the controller is away, the gate holds `/mcp` and `/front` and dials
again. A request that provably never ran reaches the controller once, when it
is back. A request cut after it was sent is an error to the client, and the gate
never sends it again. Once the hold runs out, the gate answers `503` with
`Retry-After: 5`. Over `--controller` the hold is 30 s. On `--front-socket` a
refused connect holds for 5 s, as any client's does, and a missing socket answers
at once, since systemd keeps the socket through a restart.

With `--controller tls://HOST:PORT`, `--controller-ca`, `--controller-cert`,
`--controller-key` and `--front-key` (`MOOR_GATE_FRONT_KEY`), the gate dials the
controller's `ledger_tls` listener with a front hello, so it needs no shared
volume. The key is the content of `fronts.authgear.remote.key_file`. The chart
sets these with `gate.overTls`.

The CLI and the fronts dial again only when a request provably did not run:

- The connection dropped before the controller's hello, or the controller
  answered `restarting` before any progress or output. Both prove a restart,
  so the client dials again for up to 60 s of wall time.
- The socket path exists but refuses the connection. A stale socket left by a
  controller that died does this, and so does a controller between unlink
  and bind. The client dials again for at most 5 s.
- A socket path that does not exist fails at once, with the unreachable
  message.

A `wait` or `wait-events` is sent again even after progress, because it
changes nothing. A `watch` resumes after the last cursor it delivered. The
client reports an error and never sends a request again when its write
failed, its connection dropped after the send, or it already showed progress
or output. Such a request may have run. For the CLI,
`MOOR_CONTROLLER_REDIAL_SECS` sets the 60 s limit. A value of `0` turns
redialing off. A value that is not a whole number of seconds is a usage
error.

Over ssh, `moor controller attach` applies the same rules on the remote side.
It exits 75 when the remote socket refuses the connection, and the CLI dials
again for at most 5 s. It exits 1 when the remote socket is missing, and ssh
exits 255 when it cannot connect. Both fail at once.
