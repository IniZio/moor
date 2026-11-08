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

1. Add the gate's front to `~/.config/moor/controller.yaml`:

   ```yaml
   fronts:
     authgear: {}
   ```

2. Enable its socket and restart the controller:

   ```sh
   systemctl --user daemon-reload
   systemctl --user enable --now moor-front@authgear.socket
   systemctl --user restart moor.service
   ```

3. Start the gate:

   ```sh
   moor-gate \
     --front-socket ~/.local/state/moor/fronts/authgear/front \
     --issuer https://ISSUER \
     --client-id CLIENT_ID \
     --mcp-resource-url https://HOST/mcp \
     --public-url https://HOST
   ```

   The gate listens on `0.0.0.0:8443` without TLS. Put your TLS ingress in
   front of it, and let it hold a response stream open for hours. Run the gate
   under your service manager so it restarts with the host.

4. Check it from the host:

   ```sh
   moor doctor --public-url https://HOST
   ```

Teammates can now follow [Use a team moor](use.md).

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
`moor-notify` and `moor-machine-runner` come in the release image, not as
release binaries. On a host, build each from a checkout, such as with
`cargo install --path crates/moor-notify`.

## Add Slack

Follow [Slack app](slack.md). It adds a `slack` front the same way.

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
