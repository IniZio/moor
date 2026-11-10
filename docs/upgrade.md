# Upgrade and back up

Upgrade moor in place without dropping teammates' connections, and keep the
two files that hold every secret.

## Upgrade on one host

1. Install the new binaries over the old ones:

   ```sh
   curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | sh
   ```

2. Restart the controller service, not its sockets:

   ```sh
   systemctl --user restart moor.service
   ```

3. Restart `moor-gate` and any front you run yourself.

Restarting a socket unit closes the socket, and every waiting client loses its
connection. Restarting only the service keeps them waiting in the socket's
backlog until the new controller answers.

On the restart, the controller drains for up to `drain_timeout_secs`, 30 by
default:

- It stops accepting connections. New ones wait in the backlog.
- A `wait`, `wait-events`, or `watch` ends with a `restarting` error, and the
  client dials again.
- A `start` or `retry` is cut. The new controller picks up its setup.
- Any other request in flight runs to its end.

Keep `TimeoutStopSec` in `moor.service` above `drain_timeout_secs`, so systemd
does not kill the controller mid-drain. `moor up` writes 660 seconds.

Boxes reach GitHub and Anthropic through one shared egress Proxy, `moor proxy
serve-egress`, which the controller starts. It drains and exits when the
controller exits, and the new controller starts it again on the new build.

While the controller is away, the gate holds `/mcp` requests and dials again.
When the hold runs out, it answers `503` with `Retry-After: 5`.

## Upgrade on Kubernetes

```sh
helm upgrade moor oci://ghcr.io/inizio/charts/moor --version VERSION \
  --namespace moor -f values.yaml -f secrets.yaml
```

The controller pod is replaced, not rolled, so Boxes lose the controller for
the drain plus the new pod's start. [Kubernetes](kubernetes.md#keep-boxes-up-through-upgrades)
shows how to keep the gate, the egress Proxy, and the machine runner up
through it.

## Upgrade machine agents

Each paired machine runs `moor-machine-agent`. When the controller needs a
newer agent, the agent's service exits and names the command:

```sh
moor-machine-agent update --version VERSION
```

## Store migrations

Each controller migrates the store to its own schema when it starts. The
migrations only go forward. To migrate before the first controller of a
deploy starts, run `moor store migrate` with the same `MOOR_STATE_DIR` and
`MOOR_DATABASE_URL`. An older moor opens a newer store unchanged and warns.

## Back up

The controller keeps its state in `moor.db` in the state directory,
`~/.local/state/moor` by default. It encrypts each user's secrets with
`vault.key`, next to the controller config in `~/.config/moor`.

- Back up `moor.db` and `vault.key`, and keep the two copies apart. Neither
  file alone gives a secret back.
- If you lose `vault.key`, every stored secret is lost. Each person then links
  their secrets again.
- With `MOOR_DATABASE_URL` set, back up the Postgres database in place of
  `moor.db`.
- Artifacts, Reports, and transcripts live under the state directory, or in
  the bucket that `blobs` names.

## Move the store to Postgres

Stop the controller with `systemctl --user stop moor.service`. Copy the SQLite
store into an empty Postgres database:

```sh
moor store copy --to postgres://USER:PASSWORD@HOST/moor
```

Set `MOOR_DATABASE_URL` in the controller's environment and restart it. moor
reads the URL only from the environment, never from a config file.
