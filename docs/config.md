# Configuration

moor reads three YAML files and rejects unknown keys in each. An empty file
means every key takes its default.

| File | Who writes it | Where it is |
| --- | --- | --- |
| [User config](#user-config) | The operator, or `moor up` | `$MOOR_CONFIG`, else `$XDG_CONFIG_HOME/moor/config.yaml`, else `~/.config/moor/config.yaml` |
| [`.moor.yaml`](#mooryaml) | The repo | The repo root, read through GitHub at the Box's base commit |
| [Controller config](#controller-config) | The operator, or `moor up` | The path given to `moor controller serve --config` |

The controller reads the user config of the host user it runs as. On a team
moor, that file holds the operator's defaults for everyone.

## User config

### `box`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `box.size` | `small`, `medium`, `large`, `xlarge` | none | The Box size. `.moor.yaml` wins. With no size, the provider decides. |
| `box.node` | string | the only node | The default shared node. Required when `nodes` has more than one entry. |
| `box.also` | list of `OWNER/NAME` | none | Repos a new Box clones beside its first when the start names none. |
| `box.sleep_after` | duration, such as `45m` | `5m` | How long a Box stays idle before it sleeps. |
| `box.sleep_after_blocked` | duration | `5m` | How long a Box waits on an unanswered prompt before it sleeps. The shorter of the two applies. |
| `box.max_awake` | duration | `2h` | How long a Box stays awake after a start or wake, even while its agent works. A message wakes it and the turn resumes. |
| `box.launch_timeout` | duration | `3m` | How long the agent may take to start before moor records `launch_timed_out`. |

### Nodes

A shared node is open to every user:

```yaml
nodes:
  fast:
    runtime: external
    labels: [big]
    max_boxes: 8
box:
  node: fast
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `nodes.<name>.runtime` | string | required | The crabbox provider the node's Boxes run on. |
| `nodes.<name>.labels` | list of strings | none | Labels a repo may require. |
| `nodes.<name>.max_boxes` | integer, at least 1 | unlimited | The most live Boxes on the node. Free memory may allow fewer. |

A node name matches `[a-z][a-z0-9-]*`. Without a shared node, Boxes start only
on personal nodes. Personal nodes need no config. See
[Add your machine](add-machine.md).

### `drivers`

`drivers.<provider>` holds settings for one crabbox provider.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `lookup_timeout_secs` | integer | 5, or 20 for `external` | How long moor waits for crabbox to find a Box's ssh target. |
| `strip_env` | list of names | empty | Variables the provider puts in the Box that moor removes from every command. |
| `cache_volumes` | boolean | `true` | `false` starts every Box on the provider cold. |

### `box_credentials`

Secret names a start checks for, beyond the ones the Proxy rules need. A
`.moor.yaml` can list names too. moor never puts these secrets in the Box. A
secret reaches a request only through a Proxy rule.

### `mcp_servers`

`mcp_servers.<name>` gives every Box's agent an MCP server. Give `command`,
with `args` and `env`, for a stdio server, or `url`, with `type` and
`headers`, for a remote one.

One `headers` value may hold one `${NAME}`, a secret of the Box's user. The
agent sees `moor-proxy` in its place, and the Proxy sets the real value on
requests to the URL's host. The `url` must then be `https://`.

```yaml
mcp_servers:
  linear:
    url: https://mcp.linear.app/mcp/readonly
    headers:
      Authorization: "Bearer ${linear-api-key}"
```

A person adds servers of their own with `/moor mcp add` in Slack or
`moor user mcp add`. Theirs replace a shared server of the same name. A server
added without a key signs in over OAuth. The controller registers itself with
each authorization server unless `oauth.clients` names a client for its host:

```yaml
oauth:
  callback_url: https://moor.example.com/oauth/callback
  clients:
    auth.example.com:
      client_id: moor
```

### Per-user agent profile

A person names a GitHub repo as their agent profile with
`moor user profile set https://github.com/OWNER/NAME[#REF]` or
`/moor profile set`. Each new Box of theirs applies it. The repo may hold:

| Path | What it does |
| --- | --- |
| `moor-profile.yaml` | Model, plugin marketplaces, plugins, and `env` per agent. |
| `AGENTS.md` | Instructions for any agent. |
| `skills/<name>/SKILL.md` | Agent Skills. |
| `claude/settings.json` | Merged into `~/.claude/settings.json`. |
| `codex/config.toml` | Merged into `~/.codex/config.toml`. |
| `setup.sh` | Runs once per new Box, as the Box user, within 10 minutes. |

An agent's `env` sets variables its process starts with. `{home}` and
`{profile}` expand to the Box user's home and the profile checkout. Nothing
else expands. Names that look like credentials, and `PATH`, are dropped with a
warning. The repo's `env` wins over the profile's.

```yaml
claude:
  env:
    BUN_OPTIONS: "--preload={profile}/claude/no-agent-summary.js"
```

### Machines

`machines.service` routes a Box's requests for `host` to the machine runner,
and the Proxy adds headers that name the Box and its machine:

```yaml
machines:
  service:
    host: machine.moor.internal
    upstream: 127.0.0.1:7700
```

`machines.sync` sets what a sync to a tool machine leaves alone, for every
repo. `presets` is `true`, `false`, or the presets to drop. The presets are
`node`, `cocoapods`, `gradle`, `xcode`, `dart`, and `rust`. `exclude` adds up
to 64 rsync patterns.

```yaml
machines:
  sync:
    presets: [rust]
    exclude: [android/app/build]
```

### `proxy`

`proxy.rules.<name>` defines a Proxy rule. A rule named after a default rule
adds to it.

| Key | Meaning |
| --- | --- |
| `hosts` | The exact hosts the rule serves. |
| `credential` | The secret of the Box's user that the rule injects. |
| `inject.header` | The header that carries it. `Authorization` by default. |
| `inject.form` | The header value. `{secret}` stands for the secret. |
| `inject.basic_user` | Send the secret as the HTTP Basic password for this user. |
| `allow` | `*`, or `METHODS /path` entries. `{repo}` is a repo of the Box, and `{any_repo}` is any `owner/name`. |
| `box_env` | Variables for the Box. Values can use `{repo}`. |
| `requires_repo` | Drop the rule for a Box with no repo. |

The default rules are `claude` for `api.anthropic.com` and `github-api`,
`github-git`, and `github-uploads` for GitHub. They are in
[defaults.yaml](../crates/moor-core/src/config/proxy/defaults.yaml).
[The Proxy and secrets](proxy.md) explains what they allow.

### `cache`

The `.moor.yaml` [`cache` keys](#cache-volumes), applied to every Box. A
volume declared here is kept per user, across all their repos.

```yaml
cache:
  presets: [podman]
  volumes:
    - name: cargo-bin
      path: ~/.cargo/bin
```

### Other keys

| Key | Default | Meaning |
| --- | --- | --- |
| `agent.model` | none | The default agent model. `moor start --model` wins. |
| `agent.install` | `true` | Install the agent in a Box that lacks it. |
| `dotfiles.path`, `dotfiles.install` | none | A dotfiles directory and its install script, run on each new Box. |
| `tools.crabbox`, `tools.claude`, `tools.ssh`, `tools.git` | the name on `PATH` | The program moor runs for each tool. |
| `controller.socket` | none | Send CLI commands to the controller on this socket. |
| `controller.ssh` | none | Send CLI commands over ssh to `moor controller attach`. |

moor refuses `credentials`, `github`, and `proxy.direct` in a user config.
Secrets belong to each user, and a host that no rule serves is already direct.

## `.moor.yaml`

| Key | Type | Meaning |
| --- | --- | --- |
| `brief` | string | Notes the agent reads before it starts. |
| `setup` | list of commands | Run on each new Box. A failing line puts the Box in `setup_failed`. |
| `teardown` | list of commands | Run when the Box closes. |
| `env` | map | Plain variables for the Box. `PATH` is reserved. Credential-like names never reach the agent. |
| `box.size` | `small`, `medium`, `large`, `xlarge` | 4, 8, 16, or 32 GiB. Wins over the user config. |
| `node.labels` | list | Labels the node must carry. |
| `machine.labels`, `machine.ports` | lists | The tool machine a Box claims by default, and the ports to forward. |
| `machine.sync.presets`, `machine.sync.exclude` | as `machines.sync` | Paths a sync leaves alone, added to the user config's. |
| `checkpoint.across_commits` | bool | `false` keeps saved setup per commit. `true` by default. |
| `cache` | map | See [Cache volumes](#cache-volumes). |
| `box_credentials` | list | Secrets the user must hold for a start. |
| `compose.project_prefix` | string | A prefix for compose project names. |

moor reads the file at the Box's base commit and keeps that copy for the Box's
life. [Set up a repo](repo.md) shows the keys in use.

### Setup checkpoints

On a provider with checkpoints, moor saves the Box after the clone and after
the last `setup` line. It also saves after each line that runs over 20 seconds. One more
layer above them holds the dotfiles, the agent install, and the profile. Each
layer's key covers the user, the repo, the machine class, the cache plan, and
the text of every line up to it. No key holds a commit unless
`checkpoint.across_commits` is `false`.

A new Box forks the deepest layer whose key matches. At the same commit, it
runs only the lines above that layer. At another commit, it resets tracked
files, keeps ignored ones, and runs every line again. A chain lives 7 days and
rolls forward 8 times at most. `moor start --cold` skips it.

The events `warm_started`, `warm_rerun`, `checkpoint_skipped`, and
`checkpoint_evicted` record what happened.

### Cache volumes

A cache volume is a directory the provider keeps across Boxes. Each Box gets a
private copy of the newest save. Closing the Box saves its copy.

```yaml
cache:
  presets: [docker]
  work_tree: {size_gb: 60}
  volumes:
    - name: android-sdk
      path: /opt/android-sdk
      size_gb: 12
      restore:
        - rm -rf /opt/android-sdk/.temp
```

| Key | Default | Meaning |
| --- | --- | --- |
| `cache.presets` | `true` | `false` drops every preset. A list names the presets to drop. |
| `cache.work_tree` | `true` | Keep the repo's work tree, so ignored build outputs survive. `{size_gb: N}` sizes it. |
| `cache.volumes[].name` | required | `[a-z][a-z0-9-]{0,31}`, unique in the Box. |
| `cache.volumes[].path` | required | `~/<dir>`, `/opt/<dir>`, `/var/cache/<dir>`, `/var/lib/containerd`, or `/var/lib/containers`. |
| `cache.volumes[].size_gb` | provider's | A size hint. |
| `cache.volumes[].restore` | empty | Commands run first in a Box that restored the volume. A failure empties it. |

A layer declares at most 8 volumes, with no shared names and no nested paths.
moor refuses the paths it writes for each Box. Those include `~/.ssh`,
`~/.claude`, `~/moor`, and `~/.gradle` itself.

| Preset | Paths |
| --- | --- |
| `cache` | `~/.cache`, which also holds npm, Yarn, Go builds, pip, and ccache |
| `pnpm` | `~/.local/share/pnpm/store` |
| `bun` | `~/.bun/install/cache` |
| `docker` | `~/docker`, `/var/lib/containerd` |
| `podman` | `~/.local/share/containers` |
| `gradle` | `~/.gradle/caches`, `~/.gradle/wrapper` |
| `dart` | `~/.pub-cache` |
| `rust` | `~/.cargo/registry`, `~/.cargo/git`, `~/.rustup` |
| `go` | `~/go/pkg/mod` |
| `maven` | `~/.m2/repository` |
| `cocoapods` | `~/.cocoapods/repos` |
| `bundler` | `~/.bundle/cache`, `~/.local/share/gem` |
| `android` | `~/.android/cache`, `~/.android/build-cache` |
| `android-sdk` | `/opt/android-sdk` |

A preset, the work tree, and a `.moor.yaml` volume are kept per user and repo.
A user config volume is kept per user. Another user's Box never sees a cache.

The kept work tree lives at `~/moor/repos/<repo name>` in every Box of the repo.
On a new Box, moor fetches, checks out the Box branch, resets tracked files,
and removes untracked ones. Ignored files stay. Any failure falls back to a
fresh clone.

## Controller config

`moor controller serve --config FILE` reads this file. `vault.key` lives in the
same directory.

| Key | Default | Meaning |
| --- | --- | --- |
| `socket` | `<state dir>/controller.sock` | The controller socket. |
| `state_dir` | the CLI's state directory | The state directory. |
| `settle_every_secs` | 5 | Seconds between passes that update each Box's agent state. |
| `close_setup_failed_after_secs` | 86400 | How long a Box may stay `setup_failed` before moor closes it. 0 keeps it. |
| `workflow_min_interval_secs` | 3600 | The shortest gap a workflow cron may leave between firings. |
| `drain_timeout_secs` | 30 | How long a SIGTERM waits for requests in flight. |
| `blobs` | `kind: fs` | Where artifacts, Reports, and transcripts live. See [Blob storage](#blob-storage). |
| `fronts` | empty | Fronts, each with its own socket. See [`fronts`](#fronts). |
| `ledger_tls` | none | Serve the ledger over TLS for pods that run apart from the controller: `listen`, `cert`, `key`, and `client_ca`. |

The controller refuses a `web`, `slack`, or `repos` key.

### `fronts`

`fronts.<namespace>` gives one front its own socket at
`<state dir>/fronts/<namespace>/front`. With a `command`, the controller runs
the front and restarts it when it exits.

```yaml
fronts:
  authgear: {}
  notify:
    command: [moor-notify]
    webhooks: true
```

| Key | Default | Meaning |
| --- | --- | --- |
| `command` | none | The program and arguments to run. Without it, start the front yourself. |
| `state_dir` | none | The one directory the front may write. |
| `read_only` | empty | Real paths outside the system directories that the front may read. |
| `ports` | empty | TCP ports forwarded from the host's `127.0.0.1` to the front. |
| `proxy_rules` | empty | Rules the front Proxy serves to the front. The only one is `claude`. |
| `webhooks` | `false` | Whether the front delivers users' webhooks. |
| `remote.key_file` | none | For a front in another pod, the key its hello presents to `ledger_tls`. |

The gate uses the `authgear` namespace. On Linux, a front the controller runs
gets its own user, mount, PID, and network namespaces and a root built from an
allow-list. It reaches the network only through an egress proxy that refuses
private addresses. A host that forbids unprivileged user namespaces or lacks
Landlock starts no front, and the controller logs `fronts not started`.

### Blob storage

```yaml
blobs:
  kind: s3
  endpoint: https://blobs.example
  bucket: moor
  prefix: nightly
  credentials_dir: /var/lib/moor/blobs
```

| Key | Default | Meaning |
| --- | --- | --- |
| `kind` | `fs` | `fs` keeps blobs in the state directory. `s3` uses a bucket. |
| `endpoint`, `bucket` | required | The S3-compatible endpoint and an existing bucket. |
| `region` | `us-east-1` | The signing region. |
| `prefix` | none | Put before every key, so controllers can share a bucket. |
| `credentials_dir` | none | Holds `access_key_id` and `secret_access_key`. Else moor reads `MOOR_BLOBS_ACCESS_KEY_ID` and `MOOR_BLOBS_SECRET_ACCESS_KEY`. |

### The store database

The controller keeps its state in `moor.db` in the state directory. To use
Postgres, set `MOOR_DATABASE_URL` to a `postgres://` URL in the environment.
moor never reads it from a file.

- `MOOR_DATABASE_READERS` caps read connections, 4 by default.
- The URL takes `sslmode` and `sslrootcert`, as libpq does.
- One controller runs on a database at a time. The next waits on an advisory
  lock and takes over when the holder exits.
