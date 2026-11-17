# Box images

A Box has two places, each a container, and moor-machine-agent provides both on a Docker host.

| Place | Image | Runs | Notes |
| --- | --- | --- | --- |
| agent | `images/agent` | Claude Code, herdr-core, `moor-egress-forward`, tinyproxy | Not privileged. Owns the network namespace. User `agent`, uid 1000. |
| work | `images/work` | Where Claude's Bash runs: repos build, services run, Docker runs inside | Privileged. `--network container:<agent>`. User `agent`, uid 1000. |

The places share two volumes. The work tree is mounted at `/home/agent/moor` in both, so a path
means the same file in both. The run volume is mounted at `/run/moor` in both.

Build the agent image from the repository root, because it compiles `moor-egress-forward`:

```sh
docker build -f images/agent/Dockerfile -t moor-agent .
docker build -t moor-work images/work
```

The script `tools/box-smoke.sh` builds both images and starts a Box from them. It checks the contract below on a real Docker host. CI runs it.

## `/run/moor`

| Path | Written by | Meaning |
| --- | --- | --- |
| `ssh/id`, `ssh/id.pub` | agent init | The key `box-shell` logs in with. `moor-work-init` authorizes it. |
| `bin/` | agent init | `moor-work-init` and `box-rebuild`, copied from the agent image at each agent start. Any work image with bash and sshd works, because it gets these from here. |
| `agent.ready` | agent init | The agent place has made the key and `bin/`. Start the work place after it. |
| `work.ready` | work init | sshd is listening. |
| `box.env` | machine agent | `export` lines. Every login shell in either place reads them, and dockerd starts with them. |
| `ca.pem` | machine agent | Trusted in the work place's system store. |
| `notes` | machine agent | One note per line. `box-shell` prints each unseen line to stderr, once, before the command. |
| `oom` | work init | Count of OOM kills in the work place's cgroup. Only grows, also across work place restarts. |
| `rebuild`, `rebuild.asked` | `box-rebuild`, work init | `keep` or `clean`, then the reason. Work init renames `rebuild` to `rebuild.asked` as it exits 75. The provider clears it. |
| `tmp/` | | `TMPDIR` in both places, because Claude writes the command's cwd file there and reads it in the agent place. |

## `box-shell`

Claude Code runs it as `CLAUDE_CODE_SHELL_PREFIX`, so it gets each shell line as one argument.

- `box-shell -c LINE` always runs LINE in the work place, over ssh to `127.0.0.1:2222`.
- `box-shell LINE` runs Claude's Bash tool lines in the work place. Those are the lines that source a shell snapshot.
- Any other line, such as a hook or a plugin command, runs in the agent place.
- If nothing listens on 2222 it prints "the Box is starting again" and exits 75.
- The remote command runs in its own process group, and a watcher ends the group three seconds
  after the ssh session goes away. Plain ssh without a pty would leave the command running.
- If the OOM count grew while the command ran, it says so on stderr.

## The network namespace

The agent place owns it. Restarting the agent place gives it a new namespace. The running work place stays in the old one. Its sshd still listens there, but nothing can reach it. Restart the work place after the agent place. The provider does this. Docker restarts nothing (`--restart no`).

The network must carry the MTU of the host's route to the internet (a tailscale route is 1280),
or large packets vanish. `moor-work-init` sets the inner dockerd's MTU to the MTU of `eth0`, so
creating the Docker network with the probed MTU is enough.

## `moor-work-init`

The work image's entrypoint, at `/run/moor/bin/moor-work-init`. It does these steps in order.

1. Create user `agent` if the image lacks it.
2. Write `/etc/profile.d/00-moor.sh`. It puts `/run/moor/bin` on PATH and reads `box.env`.
3. Install the CA.
4. Remove the stale `/var/run/docker.pid`. A committed image keeps the one it was committed with.
5. Start dockerd and sshd.
6. Mirror the OOM count.

| Signal or exit | Meaning |
| --- | --- |
| `moor-work-init --env` | Tells the running init (SIGUSR2) to re-read `box.env` and `ca.pem` and restart dockerd. |
| exit 75 | `box-rebuild` asked for a fresh work place, or SIGUSR1 arrived. Read `rebuild.asked` for why. |
| exit 143 | SIGTERM, so a stop. |
| exit 1 | sshd ended, or the agent place never made the key. |
