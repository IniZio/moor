# Run on one host

Take a host that [Self-host on one machine](run.md) set up and open it to
teammates. You add the gate behind your sign-in provider, then any other
fronts. You need a DNS name for the host, written here as `HOST`, and TLS in
front of port 8443. You also need an OIDC client, as
[Sign-in with OIDC](oidc.md) describes.

## Know the units

`moor up` runs the controller as systemd user units:

- `moor.socket` holds the controller socket, `~/.local/state/moor/controller.sock`.
- `moor.service` runs `moor controller serve --config ~/.config/moor/controller.yaml`.
- `moor-gate.service` runs `moor-gate` for Boxes only. It has no public
  listener and no sign-in. It serves `report_step` at `127.0.0.1:8444`.

systemd holds each socket, so a client that connects during a restart waits
and gets its answer from the new controller. Each front gets a socket of its
own, from the template unit `moor-front@.socket`. `moor up` writes no front
socket, so write the template once,
`~/.config/systemd/user/moor-front@.socket`:

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

Set `XDG_STATE_HOME` to the same value in your login shell and in the systemd
user manager, or leave it unset in both. Otherwise the CLI dials a socket the
units never bound.

## Add the gate

1. Check that `~/.config/moor/controller.yaml` has the gate's front. `moor up`
   writes it, so a rerun adds it to a config from before:

   ```yaml
   fronts:
     authgear:
       step_reports: true
   ```

2. Enable its socket and restart the controller:

   ```sh
   systemctl --user daemon-reload
   systemctl --user enable --now moor-front@authgear.socket
   systemctl --user restart moor.service
   ```

3. Give `moor-gate.service` its public listener. A drop-in survives a rerun of
   `moor up`, which rewrites the unit. Write
   `~/.config/systemd/user/moor-gate.service.d/public.conf`:

   ```ini
   [Service]
   ExecStart=
   ExecStart=%h/.local/bin/moor-gate \
     --front-socket %S/moor/fronts/authgear/front \
     --issuer https://ISSUER \
     --client-id CLIENT_ID \
     --mcp-resource-url https://HOST/mcp \
     --public-url https://HOST
   ```

   Then run `systemctl --user daemon-reload` and
   `systemctl --user restart moor-gate.service`. The gate still reads the
   service key from `moor up`'s unit, and still serves Boxes at
   `127.0.0.1:8444`. It listens on `0.0.0.0:8443` without TLS. Put your TLS
   ingress in front of it, and let it hold a response stream open for hours.

4. Check it from the host:

   ```sh
   moor doctor --public-url https://HOST
   ```

Teammates can now follow [Get started](start.md).

## Add the webhook front

So that teammates' webhooks get delivered, let the controller run
`moor-notify`:

```yaml
fronts:
  notify:
    command: [moor-notify]
    webhooks: true
```

Enable `moor-front@notify.socket` and restart the controller, as for the gate.
`moor-notify` comes in the release image, not as a release binary. On a host,
build it from a checkout, such as with `cargo install --path crates/moor-notify`.
`moor-machine-runner` is a release binary, and the install script puts it in
`~/.local/bin` with `moor`.

## Take machines from teammates

Teammates pair machines through the gate, at `https://HOST/pair`. The controller
reaches them through the machine runner. Run it beside the controller, as the
same user:

```sh
moor-machine-runner --socket ~/.local/state/moor/runner/runner.sock \
  --state-dir ~/.local/state/moor-machine-runner --agents 0.0.0.0:7701
```

Route `https://HOST/machines/link` to port 7701 on your ingress. The gate
serves `/machines/pair` and `/pair` itself.
