# Self-host on one machine

In this tutorial, you set moor up on one Linux host and start a Box there. It
takes three commands. No sign-in provider, DNS name, or TLS is needed for local
use. You add those later, when teammates join.

You need:

- a Linux host where your user can open `/dev/kvm`
- a `-moor` build of crabbox on `PATH`, as
  [Set up the Box provider](providers.md#install-crabbox) shows
- `claude` and `gh` on your own computer, signed in

## 1. Install moor

```sh
curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | sh
```

The script puts `moor`, `moor-gate`, and `moor-egress-forward` in
`~/.local/bin`. Make sure that directory is on your `PATH`.

## 2. Set the host up

```sh
moor up
```

`moor up` prints one line per step, each `ok`, `warn`, or `fail`. A failed
line carries its fix. Rerun `moor up` after a fix. A rerun changes nothing that
is already in place.

1. It checks that crabbox selects a provider and that `crabbox doctor` passes
   for it. When crabbox selects none, it prints the commands that install the
   cloud-hypervisor provider.
2. It writes the controller config, `~/.config/moor/controller.yaml`.
3. It adds a node named `local` to `~/.config/moor/config.yaml`.
4. It installs and starts the controller as the systemd user units
   `moor.socket` and `moor.service`.
5. It creates your moor user and asks for your `claude` and `github` secrets.
   Paste what `claude setup-token` and `gh auth token` print.
6. It runs `moor doctor` and prints the next command to run.

If a line warns that the controller stops when you log out, run
`loginctl enable-linger` once.

To skip the prompts in step 5, set `CLAUDE_CODE_OAUTH_TOKEN` and `GH_TOKEN`
before you run `moor up`.

## 3. Check the node

```sh
moor node ls
```

You see the `local` node, its runtime, and whether warm starts are on.
`moor up` sizes it at one Box per 4 GiB of memory beyond the first 4 GiB. Pass
`moor up --max-boxes N` to set another cap.

If `moor doctor` says `no node configured`, the user config has no `nodes`
section. Add one, with the provider that `crabbox doctor` reports:

```yaml
nodes:
  local:
    runtime: cloudhypervisor
```

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
