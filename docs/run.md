# Self-host on one machine

In this tutorial, you set moor up on one Linux host and start a Box there. It
takes three commands. No sign-in provider, DNS name, or TLS is needed for local
use. You add those later, when teammates join.

You need:

- a Linux host with rootless Docker, set up with
  [Set up a rootless Docker host for Boxes](rootless-docker.md). moor refuses
  rootful Docker.
- `/dev/kvm` that Boxes can open, since every Box gets KVM (see
  [KVM](rootless-docker.md#kvm))
- `claude` and `gh` on your own computer, signed in

Each Box's work place runs with `--privileged` so Docker works inside it. A Box
that breaks out is your user on this host, not root. Use this setup for your own
Boxes.

## 1. Install moor

```sh
curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | sh
```

The script puts `moor`, `moor-gate`, `moor-egress-forward`,
`moor-machine-agent`, and `moor-machine-runner` in `~/.local/bin`. Make sure
that directory is on your `PATH`.

## 2. Set the host up

```sh
moor up
```

`moor up` prints one line per step, each `ok`, `warn`, or `fail`. A failed
line carries its fix. Rerun `moor up` after a fix. A rerun changes nothing that
is already in place.

1. It checks that `docker info` works for your user and names rootless Docker.
   It checks the daemon is set up for Boxes, and says what a Box that breaks
   out can reach.
2. It writes the controller config, `~/.config/moor/controller.yaml`, with a
   front for the gate that may report steps, and adds the controller to
   `~/.config/moor/config.yaml`. It also adds `moor_service`, which routes
   `moor.moor.internal` to `127.0.0.1:8444`. A workflow with a plan needs
   both. A controller config that already has `fronts`, or a `moor_service`
   that names another listener, means you run the gate yourself. moor up then
   starts none.
3. It installs and starts the controller as the systemd user units
   `moor.socket` and `moor.service`. It then starts
   `moor-gate.service`, a `moor-gate` with no public listener and no sign-in.
   It serves `report_step` to Boxes at `127.0.0.1:8444`. moor up makes a key once, in `~/.local/state/moor/service.env`. It goes to the
   controller and the gate. No other process on the host can name a Box.
4. It creates your moor user and asks for your `claude` and `github` secrets.
   Paste what `claude setup-token` and `gh auth token` print.
5. It starts `moor-relay.service`, which carries Box egress from inside the
   rootless daemon's network namespace to the controller's Proxy. It then
   starts the machine runner as `moor-runner.service`. The runner takes agent
   links on a loopback port.
6. It runs `moor node add local` with an agent home under the controller's
   state directory, `~/.local/state/moor/agent-home`. The node is shared, and
   needs no sign-in.
7. It starts the machine agent as `moor-agent.service`, run as that agent
   home. The agent makes the node's Docker Boxes. It waits for the link.
8. It runs `moor doctor` and prints the next command to run.

`moor up` picks the three ports once and keeps them in
`~/.local/state/moor/up-ports.json`.

If a line warns that the controller stops when you log out, run
`loginctl enable-linger` once.

To skip the prompts in step 4, set `CLAUDE_CODE_OAUTH_TOKEN` and `GH_TOKEN`
before you run `moor up`.

## 3. Check the node

```sh
moor node ls
```

You see the `local` node, with `docker` as how it provides Boxes. The node
takes as many Boxes as the host's free memory allows.

## 4. Start a Box

```sh
moor start hello --repo OWNER/NAME --prompt "Say hello and stop"
```

Use a repo that your `gh auth token` can push to. The command prints the Box,
and its branch `moor/ID-hello`. The first Box of a repo takes a few
minutes, because it clones and installs from scratch.

Watch the agent with `moor inspect hello`. It shows the agent's state, its
latest messages, and the end of its terminal.

## 5. Close the Box

```sh
moor close hello
```

`moor close` refuses while the Box holds work that is not pushed, and says how
to push it. `moor close hello --force` discards that work.

## Next

- To let teammates in, add the gate. Follow [Run on one host](deploy.md) and
  [Sign-in with OIDC](oidc.md).
- To stop the controller, run `moor down`. It keeps the config, the state, and
  the secrets.
- Back up `~/.config/moor/vault.key`. Without it, no stored secret can be read.
  [Upgrade and back up](upgrade.md) has the rest.
