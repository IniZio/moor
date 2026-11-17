# Add your machine

Pair a computer of your own with your team's moor. A paired machine dials out
to moor, so it needs no open port, and moor stores only its public key.

A machine serves in one of two ways:

- **As a personal node.** It runs your Boxes in Docker, and only yours. It
  needs Docker, as for [a node](providers.md).
- **As a tool machine.** It hosts no Boxes. A Box borrows it to run commands,
  such as a Mac that builds iOS apps. It needs `--allow-exec`.

## Pair a personal node

1. Install Docker, and let your user run it without `sudo`. On Linux that
   means your user is in the `docker` group. `docker info` must work.

2. Install the agent as that user:

   ```sh
   curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | sh -s moor-machine-agent
   ```

3. Pair the machine. The Machines section of the account page shows this
   line ready to copy:

   ```sh
   moor-machine-agent up --server https://HOST
   ```

   `up` checks that `docker info` works and says what it needs when it does
   not. It then prints a link and a code.

4. Open the link on any device, sign in, check that the code matches, and add
   the machine. The code lasts 10 minutes.

5. Wait for `linked: NAME, N Box slots`. `up` has installed a user service
   that keeps the agent running, a systemd user unit on Linux or a launch
   agent on macOS. It runs as you, so you manage it without root:
   `systemctl --user status moor-machine-agent`.

On Linux, run `loginctl enable-linger` once, so the service keeps running
after you log out.

The machine is now a node named after it. It shows on the account page and in
`list_nodes`, with `docker` as what it provides. A new Box of yours tries your
personal nodes first, so you need no setting to use it. If the machine is
offline, the start is refused and says since when. moor does not fall back to a
shared node.

### Docker Boxes are privileged

A Box is two containers. The agent place is not privileged. The work place,
where Claude's commands run, is a privileged container, so Docker works inside
it. A user who can run Docker is already root on the host, and a privileged
container is root with fewer fences. Pair a machine only if you accept that for
your own Boxes. Nobody else's Box runs on it. `up` says this each time it runs.

### Options

Rerun `up` with new flags to change them. It keeps the pairing.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--name NAME` | the host name | The machine's name, unique among your machines. |
| `--max-boxes N` | 4 | The most Boxes the machine holds at once. Free memory may allow fewer. |
| `--labels a,b` | none | Labels a repo can ask for in `node.labels`. |
| `--agent-image IMAGE` | this release's | The agent place's image, for development. |
| `--work-image IMAGE` | this release's | The work place's image. A repo that needs other tools builds on it. |

[Box images](../images/README.md) describes both images and how to build them.

## Pair a tool machine

Pair it with labels and `--allow-exec`:

```sh
moor-machine-agent up --server https://HOST --name mac --labels macos,ios-sim --allow-exec
```

A Box claims the machine with the `machine_claim` tool, syncs its work tree
there, and runs commands with `exec`. The agent runs those commands only
because you passed `--allow-exec`. A tool machine needs no Docker. A repo names
the labels it wants in [repo config](repo.md#use-a-tool-machine).

To give each Box its own simulator, build folder, or port, add `prepare`,
`suspend`, and `release` hooks on the machine.
[Machine tools](machines.md#hooks) describes them, with an iOS example.

## Keep the agent current

When moor needs a newer agent, the service exits and names the command:

```sh
moor-machine-agent update --version 1.2.3
```

The update checks the release's checksum and signature before it replaces the
binary. The service runs with `--auto-update`, so it installs the release the
server names by itself and restarts once no Box is starting or stopping.

## Change what a machine offers

A machine paired for another kind of Box, such as an older crabbox one, pairs
again the next time you run `up`. Open the link and confirm the code as before.

## Remove a machine

`moor-machine-agent down` stops and removes the user service. The machine stays
paired.
