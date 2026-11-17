# Set up a repo

A repo needs nothing to run in a Box. Give it a config to install
dependencies, brief the agent, or ask for a bigger machine. This page shows
the common settings. [Configuration](config.md#mooryaml) lists every key.

moor keeps one config per repo, shared by every Box of the repo. Anyone who
can push to the repo, and the operator, can edit it. The settings below are
YAML, as they were in a `.moor.yaml` file, and `moor repo config` reads and
writes them:

```sh
moor repo config get OWNER/NAME > repo.yaml
moor repo config set OWNER/NAME --file repo.yaml --expected-revision 3
```

The portal is where people edit it. Sign in to the gate and open **Repos** on
the account page. Pick or type a repo, then edit the YAML in the text box.
Save sends the revision the page read. If someone else saved first, the page
says who changed it and when, and you reload before saving again. A parse
error shows under the text box. An account that cannot push to the repo reads
the config and cannot save.

`get --json` also shows the revision and who last edited it. With
`--expected-revision`, a write fails when someone else saved first, so you
read again instead of overwriting. The text must parse, or `set` refuses it
and says which key is wrong. The MCP tools `get_repo_config` and
`set_repo_config` do the same.

## Import an existing `.moor.yaml`

A repo that has no stored config and has a `.moor.yaml` at the Box's base
commit imports the file when a person who can push to the repo starts a Box.
The start warns that the file is imported. From then on moor ignores the file.
It warns on each start whose file differs from the stored config, and names
the keys. An account that cannot push to the repo does not import: its Box
uses the file, and nothing is stored. A repo listed beside the first one
looks up and imports its own config the same way.

## What an edit reaches

An edit reaches a Box that is already running only where the setting is read
again. Everything else applies to Boxes started afterwards.

| Setting | When a change applies |
| --- | --- |
| `env` | The next `moor exec`, `moor run`, setup line, and teardown, with no restart. The agent keeps the environment it started with until it launches again. |
| `teardown` | When the Box closes. |
| `machine` | When the agent next claims the machine. |
| `compose` | The next command. |
| `brief` | The next time the agent launches. |
| `setup` | A new Box, and `moor retry` on a Box whose setup failed. |
| `box.size`, `cache` | A new Box, or one that is rebuilt on a new machine. |
| `node.labels`, `box_credentials` | A new Box. |

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

Every Box runs every line. A kept work tree, which
[Cache volumes](config.md#cache-volumes) describes, keeps ignored files from
an earlier Box on the updated tree. So keep each line safe to rerun.

A guard that checks only whether output exists can keep stale output:

```sh
[ -f dist/out ] || make dist
```

Check content instead, such as a hash of the sources:

```sh
[ "$(cat dist/sum 2>/dev/null)" = "$(cksum src)" ] || { make dist && cksum src > dist/sum; }
```

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
