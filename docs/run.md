# Self-host on one machine

In this tutorial, you set moor up on one Linux host and start a Box there. It
takes three commands. No sign-in provider, DNS name, or TLS is needed for local
use. You add those later, when teammates join.

You need:

- a Linux host with Docker that your user can run (`docker info` works)
- `/dev/kvm`, only if a repo's Boxes run Android emulators
- `claude` and `gh` on your own computer, signed in

Docker Boxes are privileged containers. Each Box's work place runs with
`--privileged` so Docker works inside it, and a user who can run Docker is root
on this host. Use this setup for your own Boxes.

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

1. It checks that `docker info` works for your user, and says that Docker
   Boxes are privileged containers.
2. It writes the controller config, `~/.config/moor/controller.yaml`, and adds
   the controller to `~/.config/moor/config.yaml`.
3. It installs and starts the controller as the systemd user units
   `moor.socket` and `moor.service`. The service also listens on the address
   of Docker's bridge network, where Boxes reach the Proxy.
4. It creates your moor user and asks for your `claude` and `github` secrets.
   Paste what `claude setup-token` and `gh auth token` print.
5. It starts the machine runner as `moor-runner.service`. The runner takes
   agent links on a loopback port.
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

If `moor up` says the user config still has the node `local`, an earlier
release wrote it for crabbox. Delete `nodes.local` from
`~/.config/moor/config.yaml` and rerun.

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
