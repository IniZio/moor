# Set up a repo

A repo needs nothing to run in a Box. Add a `.moor.yaml` at its root to install
dependencies, brief the agent, or ask for a bigger machine. This page shows
the common settings. [Configuration](config.md#mooryaml) lists every key.

moor reads `.moor.yaml` from GitHub at the Box's base commit. Push a change to
the file before you start the Box that should use it. A change the agent makes
inside a Box never changes that Box.

## Install dependencies

List the commands that make a fresh clone ready to work in:

```yaml
setup:
  - npm ci
  - npx playwright install --with-deps chromium
```

Each line runs once on a new Box, in the work tree. When a line fails, the Box
goes to `setup_failed` and the agent does not start. The start names the line
and shows the end of its log. Fix the line, push it, and retry the Box.

moor saves the Box after the clone, after each line that runs longer than 20
seconds, and after the last line. A later Box of yours on the same repo starts
from the deepest saved point and skips those lines. A Box at a newer commit
runs every line again on the updated tree. So keep each line safe to rerun.

A guard that checks only whether output exists can keep stale output:

```sh
[ -f dist/out ] || make dist
```

Check content instead, such as a hash of the sources:

```sh
[ "$(cat dist/sum 2>/dev/null)" = "$(cksum src)" ] || { make dist && cksum src > dist/sum; }
```

If a guard cannot change, set `checkpoint.across_commits: false`. Each commit
then gets its own saved setup.

## Brief the agent

`brief` is text the agent reads before it starts. Say how to run the app, what
is installed, and which test users exist:

```yaml
brief: |
  Run the app with `npm run dev`; it listens on port 3000.
  Log in as test@example.com with password "test".
```

## Set environment variables

```yaml
env:
  NODE_ENV: development
```

`PATH` is reserved. Names that look like credentials, such as `API_TOKEN`, do
not reach the agent. Credentials belong in the Proxy, as
[The Proxy and secrets](proxy.md) explains.

## Ask for a bigger Box

```yaml
box:
  size: large
```

The sizes are `small`, `medium`, `large`, and `xlarge`, with 4, 8, 16, and 32
GiB of memory.

## Run on a node with labels

```yaml
node:
  labels: [gpu]
```

The Box runs only on a node that has every label. A repo can ask for labels
but never names a node.

## Use a tool machine

To have Boxes borrow one of the person's own machines, such as a Mac for iOS
builds, name its labels and the ports to forward:

```yaml
machine:
  labels: [macos]
  ports: [8081]
```

The agent claims the machine with `machine_claim`. Several Boxes can share it,
one per slot. Slot `k` moves each port `p` to `p + 10·k`.
[Machine tools](machines.md) covers the tools a Box gets.

## Keep more between Boxes

Common package caches, such as npm, cargo, and Docker, are kept by default.
The repo's work tree is kept too, so ignored build outputs survive. To keep
another directory, declare a volume:

```yaml
cache:
  volumes:
    - name: android-sdk
      path: /opt/android-sdk
```

To start every Box from a clean clone, set `cache.work_tree: false`.
[Cache volumes](config.md#cache-volumes) lists the presets and the rules.
