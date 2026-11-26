# Configuration

moor reads three YAML files and rejects unknown keys in each. An empty file
means every key takes its default.

| File | Who writes it | Where it is |
| --- | --- | --- |
| [User config](#user-config) | The operator, or `moor up` | `$MOOR_CONFIG`, else `$XDG_CONFIG_HOME/moor/config.yaml`, else `~/.config/moor/config.yaml` |
| [Repo config](#repo-config) | Anyone who can push to the repo | Stored in moor and edited with `moor repo config` or the portal |
| [Controller config](#controller-config) | The operator, or `moor up` | The path given to `moor controller serve --config` |

The controller reads the user config of the host user it runs as. On a team
moor, that file holds the operator's defaults for everyone.

## User config

### `box`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `box.size` | `small`, `medium`, `large`, `xlarge` | none | The Box size. The repo config wins. With no size, the provider decides. |
| `box.node` | string | the only node in `nodes` | The shared node a start that names none tries first, after your personal nodes. It may name an enrolled machine's node. |
| `box.also` | list of `OWNER/NAME` | none | Repos a new Box clones beside its first when the start names only one repo. A start that names two or more, and every workflow run, ignores it. |
| `box.sleep_after` | duration, such as `45m` | `5m` | How long a Box stays idle before it sleeps. |
| `box.sleep_after_blocked` | duration | `5m` | How long a Box waits on an unanswered prompt before it sleeps. The shorter of the two applies. |
| `box.max_awake` | duration | `2h` | How long a Box stays awake after a start or wake before an idle Box sleeps. A Box whose agent is mid-turn or waits on a background job is held past it, and moor records `sleep_held` once for each hold. |
| `box.max_awake_hard` | duration | `12h` | How long a Box stays awake after a start or wake before it sleeps whatever it is doing. It is never shorter than `box.max_awake`. The `asleep` event names `max_awake_hard` as its reason. |
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

A node listed here is changed here. The cap and labels of a machine's node are
settings in moor instead, which its owner edits, as [Boxes, nodes and
machines](nodes.md#a-node) says.

A node name matches `[a-z][a-z0-9-]*`. A machine an operator adds with `moor node
add` is a shared node too, with no entry here. Personal nodes need no config. See
[Add your machine](add-machine.md).

### `drivers`

`drivers.<provider>` holds settings for one provider.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `strip_env` | list of names | empty | Variables the provider puts in the Box that moor removes from every command. |
| `cache_volumes` | boolean | `true` | `false` starts every Box on the provider cold. |

### `box_credentials`

Secret names a start checks for, beyond the ones the Proxy rules need. A
repo config can list names too. moor never puts these secrets in the Box. A
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

A person adds servers of their own with `moor user mcp add`. Theirs replace a
shared server of the same name. A server added without a key signs in over
OAuth. The controller registers itself with each authorization server unless
`oauth.clients` names a client for its host:

```yaml
oauth:
  callback_url: https://moor.example.com/oauth/callback
  clients:
    auth.example.com:
      client_id: moor
```

### Per-user agent profile

A person names a GitHub repo as their agent profile in the portal, under
Settings, Agent profile, or with
`moor user profile set https://github.com/OWNER/NAME[#REF]`. The portal checks
the repo through GitHub and saves it only when it resolves. Each new Box of
theirs applies it. The repo may hold:

| Path | What it does |
| --- | --- |
| `moor-profile.yaml` | Model, plugin marketplaces, plugins, and `env` per agent. |
| `AGENTS.md` | Instructions for any agent. |
| `skills/<name>/SKILL.md` | Agent Skills. |
| `claude/settings.json` | Merged into `~/.claude/settings.json`. |
| `codex/config.toml` | Merged into `~/.codex/config.toml`. |
| `setup.sh` | Runs once per new Box, as the Box user, within 10 minutes. |

Each marketplace and plugin install is tried up to three times, 10 and then 30
seconds apart. If the profile cannot be fetched or a plugin still fails to
install, setup fails with the reason. The agent does not start.
`moor retry NAME` runs the setup again. A failed setup script is only a warning.

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

`machines.service` routes a Box's requests for `host` to the machine runner at
`upstream`, and every Box gets the MCP server `machine`. The Proxy adds headers
that name the Box and its machine. `host` defaults to `machine.moor.internal`.
The key is off when absent.

```yaml
machines:
  service:
    host: machine.moor.internal
    upstream: 127.0.0.1:7700
```

`machines.service` and `moor_service` are the hosts the Proxy serves itself.
Each entry sets both the route and the MCP server, so there is never one
without the other. Before it sends a request on, the Proxy drops every
`X-Moor-*` header the Box sent and adds its own. `upstream` is `host:port`.
`host` is an IP address or a DNS name that the Proxy looks up on each
connection. Every address it reaches must be loopback, private (RFC 1918),
`100.64.0.0/10` or `fc00::/7`. A public, link-local or unspecified address is
refused at load, or with a `502` when a name resolves to one.

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

### `moor_service`

`moor_service` gives every Box an MCP server named `moor`, where its agent calls
`report_step` for a workflow step. The agent in a claimed agent place also calls
`exec_in_box`, `exec_wait`, `start_box` and `close_box` there (see [fronts](fronts.md)). The Proxy routes `host` to `upstream`, the
gate's Box-facing listener, and stamps `X-Moor-Box`. The Box holds no token.
`host` defaults to `moor.moor.internal`. The key is off when absent.

```yaml
moor_service:
  upstream: 127.0.0.1:8444
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

The default rules serve `api.anthropic.com`, `api.github.com`, `github.com` and
`uploads.github.com`. Inside moor they are `claude`, `github-api`,
`github-git`, and `github-uploads`, the names a rule here extends. Their text is
[defaults.yaml](../crates/moor-core/src/config/proxy/defaults.yaml), and each
repo can replace it with its own [Proxy rules](repo.md#proxy-rules).
[The Proxy and secrets](proxy.md) explains what they allow.

### `cache`

The repo config's [`cache` keys](#cache-volumes), applied to every Box. A
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
| `tools.claude`, `tools.ssh`, `tools.git` | the name on `PATH` | The program moor runs for each tool. |
| `controller.socket` | none | Send CLI commands to the controller on this socket. |
| `controller.ssh` | none | Send CLI commands over ssh to `moor controller attach`. |

moor refuses `credentials`, `github`, and `proxy.direct` in a user config.
Secrets belong to each user, and a host that no rule serves is already direct.

## Repo config

| Key | Type | Meaning |
| --- | --- | --- |
| `brief` | string | Notes the agent reads before it starts. |
| `setup` | list of commands | Run on each new Box. A failing line puts the Box in `setup_failed`. A Box started with no setup (`moor start --no-setup`, `start_box` `no_setup`) runs none, on any machine it gets. |
| `teardown` | list of commands | Run when the Box closes. |
| `env` | map | Plain variables for the Box. `PATH` is reserved. Credential-like names never reach the agent. |
| `box.size` | `small`, `medium`, `large`, `xlarge` | 4, 8, 16, or 32 GiB. Wins over the user config. |
| `node.labels` | list | Labels the node must carry. |
| `machine.labels`, `machine.ports` | lists | The tool machine a Box claims by default, and the ports to forward. |
| `machine.sync.presets`, `machine.sync.exclude` | as `machines.sync` | Paths a sync leaves alone, added to the user config's. |
| `cache` | map | See [Cache volumes](#cache-volumes). |
| `box_credentials` | list | Secrets the user must hold for a start. |
| `compose.project_prefix` | string | A prefix for compose project names. |
| `proxy.allow` | map | Extra `METHOD /path` entries for a default Proxy rule. See [Extra Proxy entries](#extra-proxy-entries). |

A group that a `setup` line adds to the Box user, such as `usermod -aG docker`,
does not reach an agent held under `systemd --user`. The user manager keeps the
groups it started with. End such a setup with
`sudo -n systemctl restart user@$(id -u).service`, or do not rely on the new
group.

moor keeps this config per repo, in its store. [Set up a repo](repo.md) shows
the keys in use and says which changes reach a running Box.

### Extra Proxy entries

A repo config, a start's `config`, or a workflow's `config` can ask the Proxy
for GitHub writes the default rules refuse. The entries go under
`proxy.allow`, by default rule name:

```yaml
proxy:
  allow:
    github-api:
      - "PATCH /repos/{repo}/issues/comments/*"
      - "POST /repos/{repo}/statuses/*"
```

The entries join that rule for the Boxes that carry them, and for no other Box.
They are narrow, and moor refuses the config with the reason otherwise:

- The name must be a default rule. These are `claude`, `github-api`,
  `github-git`, and `github-uploads`. A repo config cannot add a host or a credential.
- Each entry is `METHOD /path`, with an upper-case method. `*` is not a method
  and not an entry.
- The path names `{repo}`, the Box's own repos. `{repo}` expands to every repo
  of the Box, the first repo and the others, one entry per repo. `{any_repo}` and
  `{repo:urlenc}` are refused, and so is `**`.

The Proxy reads these entries from config the controller stores. That is the
repo config, or the `config` of a Box or a workflow. moor does not read a config
from the repo's checkout. An agent cannot add entries by committing a file.
Use `proxy.rules` in the user config for anything wider.

### Cache volumes

A cache volume is a directory the provider keeps across Boxes. Each Box gets a
private copy of the newest save. Closing the Box saves its copy.

```yaml
cache:
  presets: [docker]
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
| `cache.volumes[].name` | required | `[a-z][a-z0-9-]{0,31}`, unique in the Box. |
| `cache.volumes[].path` | required | `~/<dir>`, `/opt/<dir>`, `/var/cache/<dir>`, `/var/lib/containerd`, or `/var/lib/containers`. |
| `cache.volumes[].size_gb` | provider's | A size hint. |
| `cache.volumes[].restore` | empty | Commands run first in a Box that restored the volume. A failure empties it. |

A layer declares at most 8 volumes, with no shared names and no nested paths.
moor refuses the paths it writes for each Box. Those include `~/.ssh`,
`~/.claude` (all but `~/.claude/plugins`), `~/moor`, and `~/.gradle` itself.

| Preset | Paths |
| --- | --- |
| `cache` | `~/.cache`, which also holds npm, Yarn, Go builds, pip, and ccache |
| `pnpm` | `~/.local/share/pnpm/store`, named in `npm_config_store_dir` and `pnpm_config_store_dir` |
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
| `tools` | `~/.bun/bin`, `~/.local/bin`, `~/.claude/plugins`: what the agent profile installs |
| `git` | `~/.local/share/git-mirror`: a bare mirror of the repo, which the clone fetches into and clones from |

A preset and a repo config volume are kept per user and repo.
A user config volume is kept per user. Another user's Box never sees a cache.

On a Docker machine, a package store such as `pnpm` or `rust` is shared by the
Boxes that run at once. The other volumes, `tools` and `docker` among them, are
copied into each Box and saved back when it sleeps or closes. The last save wins.

The `git` mirror is shared the same way. A Box locks it while it fetches the
repo's new commits and clones from it. So two Boxes never write it at once. The Box's clone keeps no link to the mirror. When the mirror
cannot be used, the Box clones from GitHub and the start warns why.

A Docker machine deletes a repo's kept volumes once no Box has used them for 30 days.
A Box's open and close count as use, and a volume a live or asleep Box holds is never
deleted. The machine logs each deletion with its size. [`--cache-max-age`](cli.md) sets the
age, and `0` turns this off.

## Controller config

`moor controller serve --config FILE` reads this file. `vault.key` lives in the
same directory.

| Key | Default | Meaning |
| --- | --- | --- |
| `socket` | `<state dir>/controller.sock` | The controller socket. |
| `state_dir` | the CLI's state directory | The state directory. |
| `settle_every_secs` | 5 | Seconds between passes that update each Box's agent state. |
| `close_setup_failed_after_secs` | 86400 | How long a Box may stay `setup_failed` before moor closes it. 0 keeps it. |
| `auto_close_every_secs` | 60 | Seconds between the passes that close Boxes nobody needs. 0 turns auto-close off. |
| `auto_close_run_after_secs` | 86400 | How long after a workflow run ends moor closes its Box. A steer or a terminal restarts it. 0 keeps run Boxes. |
| `auto_close_idle_after_secs` | 604800 | How long a one-off Box may sit idle before moor closes it. A steer, a terminal or a finished turn counts as use. 0 turns the idle close off. A one-off Box whose pull request merged or closed closes either way. |
| `workflow_min_interval_secs` | 3600 | The shortest gap a workflow cron may leave between firings. |
| `drain_timeout_secs` | 30 | How long a SIGTERM waits for requests in flight. |
| `blobs` | `kind: fs` | Where artifacts, Reports, and transcripts live. See [Blob storage](#blob-storage). |
| `fronts` | empty | Fronts, each with its own socket. See [`fronts`](#fronts). |
| `ledger_tls` | none | Serve the ledger over TLS for pods that run apart from the controller: `listen`, `cert`, `key`, and `client_ca`. |
| `github_app` | none | The GitHub App the operator created for this install. See [`github_app`](#github_app). |
| `agent_places` | none | The users who may have an agent place. See [`agent_places`](#agent_places). |
| `portal_url` | none | The portal's origin, such as `https://moor.example.com`. A run's Box gets its run page in `MOOR_RUN_URL`. The chart sets it from `host` when `gate.login` is on. |

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
| `webhooks` | `false` | Whether the front delivers users' webhooks. |
| `step_reports` | `false` | Whether the front reports a workflow step for the agent in a Box. Only a gate behind the Proxy should have it. |
| `box_tools` | `false` | Whether the front runs `exec_in_box`, `exec_wait`, `start_box` and `close_box` for the agent in a claimed agent place, as that place's user. Only a gate behind the Proxy should have it. |
| `remote.key_file` | none | For a front in another pod, the key its hello presents to `ledger_tls`. |

The gate uses the `authgear` namespace. On Linux, a front the controller runs
gets its own user, mount, PID, and network namespaces and a root built from an
allow-list. It reaches the network only through an egress proxy that refuses
private addresses. A host that forbids unprivileged user namespaces or lacks
Landlock starts no front, and the controller logs `fronts not started`.

### `agent_places`

A place is a Box with no repo that the agent waits in, started before any task. It never shows in
`moor ls`, the portal, `list_boxes` or the event stream, and it never sleeps or closes, until a
task claims it. `agent_places` lists the users who may start one. It is also the switch. With no
one listed, no place starts for anyone.

```yaml
agent_places:
  users: [alice@example.com]
```

| Key | Default | Meaning |
| --- | --- | --- |
| `users` | none | The emails of the users who may start a place, as `moor start --place`. |

The controller keeps one place waiting for each listed user. Every 10 seconds it starts a place for
a listed user who has none. So a place appears soon after the controller starts, and a new one
follows each claim. It closes a place whose machine is gone or stopped, and starts another. It never starts a
place for a user who is not listed. A restarted controller finds the place that stands.

A start that fails is not retried. If a place fails in setup or when its agent launches, it stays
open and no other place starts for its user. The controller log names it once. A person closes
it, then the next place starts. A start that is rejected, or that makes no Box, is left alone in
the same way. It stays so until the user has a new place or the controller restarts. A person can
start the next place with `moor start --place`.

These places are hidden, so an ask says what stands in their way. An `ask` that finds no idle place
to claim starts one for the question, and its answer's `no_idle_place` says why it found none. It
names a failed place and why it failed, a place still coming up and since when, a rejected start,
or that none was waiting. A failed or stuck place closes by that name, with `close_box` or
`moor close`, and the keeper starts the next one.

Each place holds one machine slot while it waits.

### `github_app`

The operator creates one GitHub App per moor install on GitHub and gives moor
its id and key. Without `github_app`, every App feature is off.

```yaml
github_app:
  id: 123456
  private_key_file: /run/secrets/github-app/private-key.pem
  webhook_secret_file: /run/secrets/github-app/webhook-secret
```

| Key | Default | Meaning |
| --- | --- | --- |
| `id` | required | The App id from the App's settings page on GitHub. |
| `private_key_file` | required | The PEM private key GitHub generated for the App. |
| `webhook_secret_file` | none | The App's webhook secret. Without it the gate's `/hooks/github` answers `404`. |

moor needs `metadata: read` and `pull_requests: read` on the App. It uses the App only
to find installations and read pull requests and collaborator permissions. Its
installation tokens ask for those two reads alone, whatever else the App holds. A controller
that cannot read a key file, finds no RSA key in it, or finds an empty webhook secret, does
not start.

Set the App's webhook URL to `<public url>/hooks/github`, which is the `publicIngress.host` when the chart sets it, with the same secret and the
`pull_request` event. The gate takes deliveries there without sign-in. The controller
checks each one's `X-Hub-Signature-256` and answers `401` to a bad or missing one.

The portal's Repo settings page then links "Install the App on a repo" and lists the
repos the App is installed on. Every signed-in person sees that list.

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
