# Add your machine

Pair a computer of your own with your team's moor. A paired machine dials out
to moor, so it needs no open port, and moor stores only its public key.

A machine serves in one of two ways:

- **As a personal node.** It runs your Boxes in VMs, and only yours. It needs
  KVM and a crabbox provider, as for [a self-hosted node](providers.md).
- **As a tool machine.** It hosts no Boxes. A Box borrows it to run commands,
  such as a Mac that builds iOS apps. It needs `--allow-exec`.

## Pair a personal node

1. Install the agent:

   ```sh
   curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | sh -s moor-machine-agent
   ```

2. Pair the machine. The Machines section of the account page shows this
   line ready to copy:

   ```sh
   moor-machine-agent up --server https://HOST
   ```

   `up` checks that `crabbox doctor` passes for the provider and prints the
   fix when it does not. Then it prints a link and a code.

3. Open the link on any device, sign in, check that the code matches, and add
   the machine. The code lasts 10 minutes.

4. Wait for `linked: NAME, N Box slots`. `up` has installed a user service
   that keeps the agent running, a systemd user unit on Linux or a launch
   agent on macOS. On Linux it also writes `moorboxes.slice`, a user slice
   that caps Box memory at the host's memory less the reserve. Rerunning `up`
   with a new `--reserve-mib` rewrites it.

On Linux, run `loginctl enable-linger` once, so the service keeps running
after you log out.

The machine is now a node named after it. It shows on the account page and in
`list_nodes`. A new Box of yours tries your personal nodes first, so you need
no setting to use it. If the machine is offline, the start is refused and says
since when. moor does not fall back to a shared node.

### Options

Rerun `up` with new flags to change them. It keeps the pairing.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--name NAME` | the host name | The machine's name, unique among your machines. |
| `--max-boxes N` | 4 | The most Boxes the machine holds at once. Free memory may allow fewer. |
| `--labels a,b` | none | Labels a repo can ask for in `node.labels`. |
| `--runtime PROVIDER` | crabbox's configured provider | The crabbox provider Boxes run on. |
| `--reserve-mib MIB` | 4096 plus a tenth of the host's memory | Memory kept for the host. Boxes get the rest. |

## Pair a tool machine

Pair it with labels and `--allow-exec`:

```sh
moor-machine-agent up --server https://HOST --name mac --labels macos,ios-sim --allow-exec
```

A Box claims the machine with the `machine_claim` tool, syncs its work tree
there, and runs commands with `exec`. The agent runs those commands only
because you passed `--allow-exec`. A repo names the labels it wants in
[`.moor.yaml`](repo.md#use-a-tool-machine).

To give each Box its own simulator, build folder, or port, add `prepare`,
`suspend`, and `release` hooks on the machine.
[Machine tools](machines.md#hooks) describes them, with an iOS example.

## Keep the agent current

When moor needs a newer agent, the service exits and names the command:

```sh
moor-machine-agent update --version 1.2.3
```

The update checks the release's checksum and signature before it replaces the
binary.

## Remove a machine

`moor-machine-agent down` stops and removes the user service. The machine stays
paired. To remove the pairing, run `/moor machine remove NAME` in Slack.
