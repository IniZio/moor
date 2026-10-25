# Configuration reference

moor reads three YAML files. moor rejects unknown keys in each file. An empty
file means every key takes its default, except `box.provider`, which has none.

| File | Who writes it | Where it is |
| --- | --- | --- |
| [User config](#user-config) | You | `$MOOR_CONFIG`, else `$XDG_CONFIG_HOME/moor/config.yaml`, else `$HOME/.config/moor/config.yaml` |
| [`.moor.yaml`](#mooryaml) | The repo | The repo's base commit, read through GitHub when the Box starts |
| [Controller config](#controller-config) | Whoever runs the controller | The path given to `moor controller serve --config` |

## User config

### `box`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `box.provider` | string | none | The crabbox provider for new Boxes. Required to start a Box: `moor start` of a new Box fails until it is set. Existing Boxes keep the provider they started on. A repo cannot set it. |
| `box.size` | `small`, `medium`, `large`, or `xlarge` | `small` | The Box size. A `.moor.yaml` value wins over this one. |
| `box.max_boxes` | integer, at least 1 | none | The most live Boxes at once. |
| `box.sleep_after` | duration such as `30s`, `45m`, `2h`, or `1d` | `30m` | How long a Box stays idle before moor puts it to sleep. |
| `box.sleep_after_blocked` | duration | `5m` | How long a Box stays idle while its agent waits on an unanswered prompt or a first-run screen before moor puts it to sleep. The shorter of this and `box.sleep_after` applies. An answer, steer or message wakes it. |
| `box.max_awake` | duration | `2h` | How long any Box stays awake, counted from its last start or wake, before moor puts it to sleep, even while its agent works. A steer or message wakes it and the agent resumes. |
| `box.launch_timeout` | duration such as `30s`, `3m`, or `1h` | `3m` | How long a Box's agent may take to start a session, or to reach a first-run screen, before moor records `launch_timed_out`, warns, and counts the Box as quiet so it can sleep. Restart the agent to try again. moor also stops restarting an agent on its own after 3 relaunches in 10 minutes: it refuses the next one with a message and records `relaunch_capped`. |

### `drivers`

`drivers.<provider>` holds settings for one provider. See
[Box providers](providers.md) for how they work.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `max_boxes` | integer, at least 1 | none | The most live Boxes on this provider. It wins over `box.max_boxes`. |
| `lookup_timeout_secs` | integer, at least 1 | 5, or 20 for `external` | How long moor waits for crabbox to look up a Box's ssh target. |
| `strip_env` | list of variable names | empty | Variables that the provider puts in the Box, which moor removes from every command it runs there. |
| `cache_volumes` | boolean | `true` | Whether this provider's Boxes get [cache volumes](#cache-volumes). `false` starts every Box on it cold, with no warning. |

### `box_credentials`

A list of secret names that a `start` checks. A `.moor.yaml` can list names
too. Each name is a secret of the Box's user and matches `[a-z][a-z0-9_-]*`. A
`start` whose user lacks one of them is rejected. A name may not be a secret
that a Proxy rule injects, such as `claude` or `github`. A `.moor.yaml` is
checked against the default rules only.

moor does not put these secrets into the Box. A secret reaches a Box's requests
only through a Proxy rule whose `credential` names it.

### `mcp_servers`

`mcp_servers.<name>` adds an MCP server for the agent. Give exactly one of
`command` or `url`.

| Key | Type | Meaning |
| --- | --- | --- |
| `type` | string | The transport, such as `http`, `sse`, or `stdio`. |
| `command` | string | The program to run. |
| `args` | list of strings | The program's arguments. |
| `env` | map | The program's environment. |
| `url` | string | The server URL. |
| `headers` | map | Request headers. |
| `allow_tools` | list of tool names | The only tools the agent may call. Every tool when unset; none when empty. |
| `coordinator` | bool | Reserved for a front's coordinator. Off by default. |

moor hands each server to the agent at launch. With `allow_tools` set, the
agent profile refuses any other tool of that server; for Claude Code, a
`PreToolUse` hook blocks it.

One `headers` value may hold one `${NAME}`, which names the Box user's secret
`NAME`. The `url` must then be `https://`. The Box never holds the secret: the
agent sees `moor-proxy` in its place, and moor adds a Proxy rule `mcp-<name>`
that sets the header with the real secret on requests to the URL's host.
`NAME` may not be a secret that another Proxy rule injects. `${NAME}` anywhere
else (`args`, `env`, `url`) is refused, because a stdio server runs in the Box
and would hold the secret.

A read-only Linear server, with each teammate's own key stored as their
secret `linear`. Create the key with read-only permissions: Linear Settings →
Security & access → Personal API keys → Only select permissions.

```yaml
mcp_servers:
  linear:
    url: https://mcp.linear.app/mcp
    headers:
      Authorization: Bearer ${linear}
    allow_tools: [get_issue, list_issues, list_comments, get_project]
```

### `proxy`

`proxy.rules.<name>` defines a Proxy rule. A rule with the name of a default
rule adds to that default.

| Key | Type | Meaning |
| --- | --- | --- |
| `hosts` | list of host names | The exact hosts the rule serves. |
| `credential` | string | The name of the Box user's secret the rule injects, matching `[a-z][a-z0-9_-]*`. |
| `inject.header` | string | The header that carries the credential. The default is `Authorization`. |
| `inject.form` | string | The header value. `{secret}` stands for the credential. |
| `inject.basic_user` | string | The HTTP Basic auth user. moor sends the credential as the password. A rule takes `form` or `basic_user`, not both. |
| `allow` | list | `*` for any request, or `METHODS /path` entries such as `GET\|POST /repos/{repo}/pulls`. In a path, `*` matches one segment and `**` matches any number. |
| `box_env` | map | Environment variables for the Box. Values can use `{repo}`. |
| `requires_repo` | boolean | If true, moor drops the rule for a Box with no repo. |

A rule whose `allow` is `*` adds its credential to every request to its
hosts, whatever the method. That includes PUT and DELETE, so for a private npm
registry it covers `npm publish` and unpublish too. A rule with no `allow`
behaves like `*`. A rule with `METHODS /path` entries adds it only when an entry matches,
and never when an upstream could read the path differently than moor does.
Such a path has an encoded `/` or `.` (`%2f`, `%2e`), a `;`, a `\`, a `%` that
is left after one decoding, or a control character. A Box's Proxy forwards that
request without the credential, and a front's Proxy refuses it. `GET /**` is a path
entry too. npm asks a registry for a scoped package as `/@scope%2fname`, so a
rule for a private npm registry needs `allow: ["*"]`.

The default rules are `claude` for `api.anthropic.com`, which injects the
`claude` secret, and `github-api`, `github-git`, and `github-uploads` for
`api.github.com`, `github.com`, and `uploads.github.com`, which inject the
`github` secret. They are in
[defaults.yaml](../crates/moor-core/src/config/proxy/defaults.yaml). A user
rule that extends a default cannot set `hosts` or `credential`. Its `allow`
entries come after the default's, and its `box_env` values win.

Only the hosts of these rules go through the Proxy. The Box reaches every
other host direct. See
[Which traffic goes through the Proxy](providers.md#which-traffic-goes-through-the-proxy).

### `cache`

The same keys as the [`.moor.yaml` `cache:` section](#cache-volumes), for
every Box this config starts. `cache.presets: false` turns the presets off for
every repo, and a list drops those presets for every repo; a repo cannot turn
them back on. `cache.volumes` declares volumes kept per user, shared across
that user's repos.

```yaml
cache:
  presets: [podman]
  volumes:
    - name: cargo-bin
      path: ~/.cargo/bin
```

### Removed keys

moor refuses a user config that has a `credentials` or `github` key. Each
moor user stores their own secrets with `moor user secret set NAME`, and the
`github-uploads` rule applies to every Box with a repo.

moor also refuses `proxy.direct` in a user config and a `proxy` key in
`.moor.yaml`. Every host that no rule serves is direct already.

### Other keys

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `agent.model` | string | none | The default agent model. `moor start --model` wins over it. |
| `agent.install` | boolean | `true` | Whether moor installs the agent in a Box that lacks it. `false` leaves the Box as its image and `setup` made it, and the start then fails if the agent is missing. |
| `dotfiles.path` | absolute path or `~/...` | none | A dotfiles directory that moor installs on each new machine. |
| `dotfiles.install` | relative path | none | The install script inside `dotfiles.path`. |
| `surface.herdr.session` | string | `moor` | The herdr session that holds Box workspaces. |
| `tools.crabbox`, `tools.herdr`, `tools.claude`, `tools.gh`, `tools.ssh`, `tools.git` | path | the name on `PATH` | The program moor runs for each tool. |
| `controller.socket` | path | none | Send CLI commands to the controller on this socket. |
| `controller.ssh` | ssh destination | none | Send CLI commands over `ssh` to `moor controller attach` on that host. Set either `socket` or `ssh`, not both. |
| `state_dir` | path | none | moor parses this key but does not use it. Use `--state-dir` or `MOOR_STATE_DIR`. |

## `.moor.yaml`

A repo's `.moor.yaml` configures its Boxes.

| Key | Type | Meaning |
| --- | --- | --- |
| `brief` | string, at most 4000 characters | Notes from the repo's author for the Box's agent: how to start services, what is installed, test users. The agent reads it verbatim under "This project's Box", marked as coming from `.moor.yaml`. |
| `setup` | list of shell commands | Commands that run on each new machine. |
| `teardown` | list of shell commands | Commands that run when the Box closes. |
| `env` | map | Plain environment variables for the Box. `PATH` is reserved. moor does not pass names that look like credentials, or names that moor sets, to the agent. |
| `compose.project_prefix` | string | A prefix for compose project names. |
| `box.size` | `small`, `medium`, `large`, or `xlarge` | The Box size. It wins over the user's `box.size`. |
| `box_credentials` | list of names | Secrets that the Box's user must hold for a `start`. moor does not put them into the Box. |
| `cache` | map | Which presets to keep and which other directories to keep across Boxes. See [Cache volumes](#cache-volumes). |

moor reads `.moor.yaml` from GitHub at the Box's base commit when the Box
starts, and keeps that copy for the Box's life. Setup, the agent, `moor run`
and the teardown all use it, so a change the agent makes to the file in the
Box never changes what moor runs. A changed `.moor.yaml` takes effect in the
next Box after it is pushed.

### Cache volumes

A cache volume is a directory in the Box that the provider keeps across Boxes,
so a second Box does not download the same packages again. Each Box gets a
private copy of the newest kept copy when it starts. When the Box closes, its
copy becomes the newest. Two Boxes never write the same copy, and the newest
close wins. The provider decides where the copies live and how big they grow;
see [Cache volumes](providers.md#cache-volumes). A provider without them starts
every Box cold. It warns `cache_unavailable` only when the repo or the operator
declared volumes of their own; presets alone start cold without a warning.

```yaml
cache:
  presets: [docker]          # drop these presets; false drops all; default: all on
  work_tree: {size_gb: 60}   # false: clone fresh in every Box; default: true, 40 GB
  volumes:
    - name: android-sdk      # [a-z][a-z0-9-]{0,31}
      path: /opt/android-sdk
      size_gb: 12            # a hint; the provider's default when unset
      seal:                  # optional; runs in the Box at close
        - rm -rf /opt/android-sdk/.temp
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `cache.presets` | `true`, `false`, or a list of preset names | `true` | Which presets this repo keeps. A list names the presets to drop. |
| `cache.work_tree` | `true`, `false`, or `{size_gb: N}` | `true` | Whether the repo's work tree is kept. See [Work tree](#work-tree). |
| `cache.volumes[].name` | string | required | The volume's name, unique in the Box. |
| `cache.volumes[].path` | path | required | The directory kept: `~/<dir>` (the Box user's home), `/opt/<dir>`, `/var/cache/<dir>`, or exactly `/var/lib/containerd` or `/var/lib/containers` (nothing below them). Letters, digits, `/` and `._-+@` only. |
| `cache.volumes[].size_gb` | integer, at least 1 | none | A size hint. The provider caps it. |
| `cache.volumes[].seal` | list of shell commands | empty | Commands that run in the Box when it closes, after `teardown`, before the provider keeps the volume. |

A layer declares at most 8 volumes. Two volumes may not share a name, and
their paths may not be equal or nested. A repo volume and an operator volume
that clash leave the Box cold, with a `cache_refused` warning. A declared
volume on, above or inside Docker's data paths (`~/docker`,
`/var/lib/containerd`) is dropped with its own `cache_refused` warning, even
when the docker preset is off, and the Box keeps the rest of its volumes.

#### The agent

When moor installs the agent in a Box (see `agent.install`), the directory that
holds the install is a volume named `agent`, kept per user and shared by every
repo that user starts a Box in. A later Box restores it and links the cached
version instead of downloading the agent again. Its seal runs the newest
cached version, so a half-finished install is never kept. A provider without
cache volumes keeps nothing and says nothing; the install runs in every Box.

#### Presets

Every Box with a repo gets these unless a layer drops them. A volume that a
repo or the operator declares on the same name or directory replaces the
preset. Paths are each tool's default on Linux, except npm and Yarn, whose
caches moor points at `~/.cache` in every command.

| Preset | Paths | Where the tool documents it |
| --- | --- | --- |
| `npm` | `~/.cache/npm` | moor sets `npm_config_cache`; [npm `cache`](https://docs.npmjs.com/cli/v10/using-npm/config#cache) |
| `yarn` | `~/.cache/yarn` | moor sets `YARN_CACHE_FOLDER` and `YARN_GLOBAL_FOLDER` below it; [Yarn `enableGlobalCache`](https://yarnpkg.com/configuration/yarnrc#enableGlobalCache) |
| `pnpm` | `~/.local/share/pnpm/store` | [pnpm `storeDir`](https://pnpm.io/settings/store#storedir) |
| `bun` | `~/.bun/install/cache` | [Bun global cache](https://bun.sh/docs/install/cache) |
| `docker` | `~/docker`, `/var/lib/containerd` | Docker's data root on every external-provider Box ([`data-root`](https://docs.docker.com/engine/daemon/)) and the containerd image store that Docker 29 uses by default ([containerd image store](https://docs.docker.com/engine/storage/containerd/)); sealed, see below |
| `podman` | `~/.local/share/containers` | [rootless `graphroot`](https://github.com/containers/storage/blob/main/docs/containers-storage.conf.5.md) |
| `gradle` | `~/.gradle/caches`, `~/.gradle/wrapper` | [Gradle user home](https://docs.gradle.org/current/userguide/directory_layout.html) |
| `dart` | `~/.pub-cache` | [`PUB_CACHE`](https://dart.dev/tools/pub/environment-variables) |
| `rust` | `~/.cargo/registry`, `~/.cargo/git`, `~/.rustup` | [Cargo home](https://doc.rust-lang.org/cargo/guide/cargo-home.html), [`RUSTUP_HOME`](https://rust-lang.github.io/rustup/environment-variables.html) |
| `go` | `~/go/pkg/mod`, `~/.cache/go-build` | [`GOPATH`/`GOMODCACHE` and `GOCACHE`](https://pkg.go.dev/cmd/go) |

The `docker` preset's seal removes every container, volume and network, so only
images and build cache are kept. It runs `docker` directly when the login user
can reach the Docker socket, else `sudo -n docker`; when neither works, the seal
fails. Without Docker in the Box there is nothing to seal.

When a volume's seal succeeds, moor writes a `.crabbox-sealed` file into each of
its paths, through `sudo -n` where the login user cannot. The provider keeps a
volume that has a seal only when every one of its paths holds that file, and it
deletes the file when it restores the volume. So a seal that fails, a close that
never ran the seal, or a Box stopped outside moor leaves the volume unkept. A
seal that fails also empties its volume in the Box, and the close goes on with a
`cache_seal_failed` warning. Each volume with a seal is kept in a store of its
own, so a docker volume that is not kept never holds back the other presets.

#### Work tree

Every Box with a repo also keeps its work tree, in a volume named `worktree`,
so a later Box of the same user and repo starts with the previous Box's
ignored build outputs (`build/`, `target/`, `.cxx`) instead of a fresh clone.
The workdir is then `~/moor/repos/<repo name>` in every Box of that repo. That
fixed path lets tools that record absolute paths, such as CMake, Gradle and
cargo, reuse their outputs, and it makes Docker Compose's default project name
the repo name.

When the Box starts on a kept tree, moor refreshes it before anything else
runs. It fetches, checks out the Box branch from the remote, or from the base
when the remote has no such branch, resets every tracked file, and removes
untracked files and nested repos. Ignored files stay. It then removes every
remote but the repo's own and deletes every ref except the Box branch, the
remote's branches and the tags, along with the reflogs. A file whose content did not
change keeps its modification time. When any step fails, moor empties the
directory and clones fresh, as on the first Box, so the start never fails
because of the kept tree. The start says which happened in a `work_tree` field of its
reply, which the CLI prints as its own line: `work tree: refreshed`, `work tree:
cold`, or `work tree: cold after: <reason>` when the refresh ran and fell back to a
fresh clone. That last case also warns, with the code `work_tree`, because
something went wrong. A Box that keeps no tree, or that has already refreshed its
tree, such as a rerun of the start, says nothing. `cold` does not say why there was
no tree to refresh. That can be a first Box, a provider that kept none, or a close
that did not keep it; the close of that Box warned `work_tree_not_kept` with the
reason.

The close keeps the tree only when it holds nothing the remote lacks. No
rebase, merge, cherry-pick, revert, am or bisect may be in progress, and there
may be no stash and no staged change. No commit may be reached by HEAD or by
any ref other than a tag or one of the remote's branches, unless one of the
remote's branches or the base holds it. That covers local branches, a detached
HEAD, notes, other remotes and custom refs. Tags do not block it, because the
refresh prunes tags the remote lacks. Modified and
untracked files do not block it, since the next refresh removes them. A tree
that is not kept stays as it is in the Box, the close warns
`work_tree_not_kept` with the reason. The next Box then starts from an earlier
kept tree, when the provider has one, or clones fresh. A non-forced close
already refuses unpushed work, so this matters only after `close --force`. A
provider may also skip saving a tree that barely grew since it was restored.

The work tree has a store of its own, so a failed Docker seal never discards
it, nor the reverse. `size_gb` is the size hint for that store. A repo whose
name has a character other than letters, digits and `._-+@` gets no work tree.
A declared volume named `worktree` replaces it.

#### Who shares a cache

| Declared by | Kept for |
| --- | --- |
| A preset, the work tree or `.moor.yaml` | One user and one repo |
| The operator's user config | One user, across all their repos |

Another user's Box never sees a cache. A repo cannot widen its own scope: the
scope follows the file a volume is declared in, and `.moor.yaml` has no key for
it. Each state directory makes its own random salt for the keys, so a fresh
state directory starts every cache cold.

#### What is never cached

moor refuses a path that is, holds, or sits inside a directory moor writes for
each Box or that holds the Box's own state: `~/.moor-*` (the Proxy CA and
moor's own files), `~/.moor`, `~/moor` (the Box workdirs), `~/.ssh`,
`~/.claude` (the agent's state), `~/.config/gh` (the gh login),
`~/.gradle/gradle.properties` and `~/.gradle/moor-proxy-truststore.p12`. So `~/.gradle` itself is refused; the
`gradle` preset keeps `~/.gradle/caches` and `~/.gradle/wrapper` beside those
files. Secrets never reach a cache: moor keeps them in the Proxy, not in the Box.

`moor env` lists each volume's name and paths. It never shows a key.

## Controller config

`moor controller serve --config FILE` reads this file.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `socket` | path | `<state dir>/controller.sock` | The controller socket. `~` is allowed. |
| `state_dir` | path | the same state directory as the CLI | The state directory. `~` is allowed. |
| `settle_every_secs` | integer, above 0 | 5 | Seconds between passes that update each open Box's agent state. |
| `drain_timeout_secs` | integer | 600 | Seconds a SIGTERM waits for in-flight requests. See [Stopping the controller](#stopping-the-controller). |
| `fronts` | map | empty | Fronts that each get their own socket, and that the controller runs when they have a `command`. |

The controller refuses a config that has a `web`, `slack` or `repos` key.

### Stopping the controller

SIGTERM drains the controller, then it exits 0:

1. It stops accepting connections. A client that connects now waits in the
   socket's backlog and fails when the controller exits. Under systemd socket
   units, the next controller answers it instead. See
   [Deploy the controller with systemd](deploy.md). One accepted at the
   moment the drain began gets an error frame of kind `restarting`.
2. A `wait`, `wait-events` or `watch` gets an error frame of kind
   `restarting` and is closed. Send it again once a controller listens.
3. Any other request in flight, such as a `start`, `close` or `run`, runs
   to its end. The controller waits up to `drain_timeout_secs` for them,
   then exits anyway. A `start` cut off this way leaves its Box to `moor close`.
4. No request is run once the drain began. Each connection ends with an
   error frame of kind `restarting` that names no request, so a request
   half sent, sent behind another, or sent just after the signal can be sent
   again.
5. No new settle pass starts. One already running finishes the Box it is on,
   such as an auto-sleep, then stops, and the drain waits for it.

A command that `moor run` streams keeps running through the drain. SIGINT
does not drain: it stops the controller at once. Under a service manager,
set the stop timeout above `drain_timeout_secs`.

### `fronts`

`fronts.<namespace>` gives one front its own socket:

```yaml
fronts:
  web:
    command: [moor-web, --port, "8080"]
    state_dir: /home/me/.local/state/moor-web
    ports: [8080]
  slack: {}
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `command` | list of strings, not empty | none | The program and arguments the controller runs as this front. Without it, start the front yourself. |
| `state_dir` | absolute path, not under `/tmp` | none | The one directory the front may write. Without it, the front writes nothing but `/dev/null`. |
| `ports` | list of TCP ports, 1 to 65535 | empty | Ports a client on the host reaches the front on at `127.0.0.1`. The front listens on the same port inside its own network namespace. Each is forwarded from the host's `127.0.0.1` only, never its other addresses, and nothing else reaches the front from outside. The controller does not guess them from `command`. On macOS the front shares the host's network, so the key changes nothing there. |
| `read_only` | list of absolute real paths | empty | Files or dirs outside the system dirs that the front may read, such as a config or token file under `HOME`. |
| `proxy_rules` | list of rule names | empty | The rules the front Proxy serves to this front. `claude` is the one rule: it injects the user's `claude` secret on `POST /v1/messages`, `GET /api/claude_code/settings` and `GET /api/claude_code/policy_limits` at `api.anthropic.com`. |

#### The front Proxy

When an entry lists `proxy_rules`, the controller runs `moor proxy serve-fronts`
as a child and starts it again with backoff whenever it exits. It listens on a
socket per front in moor's private socket directory. The front's shim binds a
loopback port in the front's network namespace and passes each connection to
that socket, and the front finds the port in `MOOR_FRONT_PROXY_PORT`. The
Proxy signs with a CA kept in `<state dir>/proxy/fronts`. The front gets its
certificate as PEM text in `MOOR_FRONT_PROXY_CA_PEM`, so no controller path is
shown to it. Each front's Proxy socket sits alone in a directory of its own,
and the shim's pid 1, which holds that directory open, is not dumpable, so the
front cannot reach it through `/proc/1/fd`. The bridge needs the Linux shim: on macOS no
front gets a Proxy, so a coordinator front there says the controller gave it
no Proxy.

A front asks for a lease with `proxy-lease`, as an identity of its own
namespace. The controller grants it only when that identity's user holds
every secret the front's rules inject, and only while the Proxy runs. A CONNECT
reaches the Proxy's nono only when its proxy password is a live lease of the
same front; nono then injects that user's secrets and refuses every host no
rule lists. The store keeps a hash of each lease, never the lease, and the
controller ends every lease when it starts.

#### Sockets and processes

For each entry the controller listens on `<state dir>/fronts/<namespace>.sock`,
mode 0600 in a 0700 directory. Under systemd, the controller takes the socket
that systemd passes as `front-<namespace>` instead of binding it. A namespace matches `[a-z][a-z0-9-]*` and is
not `local` or `proxy`, which names the front Proxy in the controller log.

When an entry has a `command`, the controller starts it at startup. The front
gets an `O_PATH` file descriptor to its own socket as fd 3, and
`MOOR_FRONT_SOCKET=/proc/self/fd/3` names it, so the front connects to that
path. The front's stdin is empty and its stdout is the controller's. Each
line it writes to stderr goes to the controller log as `front <namespace>:
<line>`.

Each front runs under `moor front-shim`, in its own user, mount, PID and
network namespaces, with the controller's uid and gid and no capabilities. There the
state dir is an empty read-only tmpfs, the controller socket and the vault key
read as `/dev/null`, and `/proc` shows only the front's own processes, so fd 3
is the front's only way to the controller. The shim is pid 1 of that
namespace: it reaps orphans, passes SIGTERM and SIGINT to the front, and exits
with the front's status. If the controller's shim dies, the kernel kills every
process in the namespace.

On Linux, each front runs in a root of its own, built from an allow-list.
Nothing else of the host exists there, so a Unix socket elsewhere, such as
the D-Bus session bus, Docker's or LXD's, cannot be reached:

- **Read and run.** `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin` and `/etc` where
  they exist, and the file `/etc/resolv.conf` points to. The front's program:
  the controller resolves `command`'s first word on its `PATH` to a real path
  and runs that path. Each `read_only` path.
- **Devices.** `/dev/null`, `zero`, `full`, `random`, `urandom` and `tty`, and
  the `fd`, `stdin`, `stdout` and `stderr` links.
- **Processes.** A fresh `/proc` that shows only the front's own processes.
- **Write.** Only the `state_dir`, its `tmp` subdirectory, which is the
  front's `/tmp`, and `/dev/null`. Every other mount is read-only, so chmod,
  chown, utimes and xattr changes fail with `Read-only file system`, even
  through a file descriptor, including the front's own socket on fd 3.
  Landlock also refuses writes outside them.
- **Network.** The front has a network namespace of its own with nothing but
  its loopback, so it has no route out. Everything outbound goes through the
  front's egress proxy, which the shim runs in the host's network and names in
  `HTTPS_PROXY`, `HTTP_PROXY` and their lowercase forms, with `NO_PROXY` set to
  the front's own loopback. The proxy takes only `CONNECT` to port 443 or 80.
  It resolves the host once and answers 403 when any address it resolves to
  lies in `0.0.0.0/8`, `10.0.0.0/8`, `100.64.0.0/10`, `127.0.0.0/8`,
  `169.254.0.0/16`, `172.16.0.0/12`, `192.0.0.0/24`, `192.168.0.0/16`,
  `198.18.0.0/15`, `224.0.0.0/4`, `240.0.0.0/4`, `::/128`, `::1/128`,
  `fc00::/7`, `fe80::/10` or `ff00::/8`, or is an address of a host interface
  (public ones included). Otherwise it connects to the addresses it checked,
  without resolving again, so a second DNS answer cannot point it elsewhere.
  It holds at most 64 tunnels per front, answers 429 past that, and logs one
  `egress:` line per decision with the host, port, address, decision and
  reason. That keeps the front off the host's loopback, its other interfaces
  (such as docker0, tailscale0 or an LXD bridge), the LAN and other fronts.
  The host's addresses are read when the front starts, so an address the host
  gains later is not denied to that front until it restarts. Only the `ports`
  the entry lists reach the front from outside, from the host's `127.0.0.1`.
  Because loopback is the front's own, a front's config cannot point at a
  service on the host's loopback: for moor-slack, an `api_base` of
  `http://127.0.0.1:...` does not work when the controller runs it.

A `read_only` path must be a real path: one with a symlink or `..` in it is
refused. So is one that is `/tmp` or lies under it, because the front's own
tmp dir is mounted there and would hide it. So is one that equals, holds or
lies inside the controller's state dir, the dir of its config file (which
holds the vault key), the vault key or the controller socket. When a system
dir above holds one of those, the controller refuses to start, as every front
could read it. It also refuses when `/etc/resolv.conf` resolves under `/tmp`.

The controller creates the `state_dir` and its `tmp` subdirectory, mode 0700,
and sets `MOOR_FRONT_STATE_DIR` to the `state_dir` and `TMPDIR` to its `tmp`.
moor-web and moor-slack keep their state in `MOOR_FRONT_STATE_DIR` when
`--state-dir` is not given.

Fronts need unprivileged user namespaces and Landlock, and no program on the
host beyond moor. When the host forbids user namespaces
(`kernel.unprivileged_userns_clone=0`, `user.max_user_namespaces=0`, or
`kernel.apparmor_restrict_unprivileged_userns=1`) or lacks Landlock, the
controller logs `fronts not started` with the reason,
starts none, and keeps serving. `moor front-shim --check` makes that call: it
enters the namespaces, brings the front's loopback up, enters the front's
root, applies the sandbox and confirms a write outside the allow-list is
refused.

On macOS the shim has no namespaces. It applies nono's Seatbelt profile, which
denies by default, to itself and the front. The front may read `/usr`, `/bin`,
`/etc` (and `/var/run/resolv.conf`), its program and each `read_only` path. It
may write only its `state_dir` and `/dev/null`. It may connect to no Unix
socket but its own and the DNS resolver's. Outbound TCP is not restricted.
`MOOR_FRONT_SOCKET` holds the socket's path, because macOS cannot pass it on
fd 3. The front shares the shim's process group. The shim kills that group
when the front exits or the controller goes away, and the controller kills it
when the drain timeout ends. There is no pid file on macOS.

Known limit on macOS: a process the front starts that calls `setsid` or
`setpgid` leaves the group. The group kill misses it, so it outlives the front
and the controller, still under the Seatbelt profile. Linux has no such gap,
because the front's PID namespace ends with it.

While a front runs, `<state dir>/fronts/<namespace>.pid` holds the shim's pid on
the first line and its start time from `/proc/<pid>/stat` on the second. When
the front exits, the controller starts it again after 0.5 s. The wait doubles
each time, up to 30 s, and goes back to 0.5 s after a run that lasted 60 s.
When the controller drains on SIGTERM, it sends SIGTERM to each front at once,
starts none again, and exits after it has reaped each front's outer shim; the rest
of that front's processes follow by death signal a few milliseconds later. A front still running
when `drain_timeout_secs` ends is killed. A front can outlive a controller that
crashed; the next controller reads each pid file and, when the process
there still has the recorded start time, sends it SIGTERM and then SIGKILL after
5 s. A pid with another start time belongs to another process and is left alone.
The next controller never adopts a front.

A moor user owns secrets, such as a Claude or GitHub token. An identity names
a person on one front, as `<namespace>:<rest>`, and a user can link several
identities with `moor user link-code`. Every request on a front socket except
`version` must assert an identity in that front's namespace. Any other request
gets `must assert an identity`. On the controller
socket, the controller reads the peer's uid from the kernel and acts as `local:<uid>`. It
refuses any other identity.

A front acts for one of its people by wrapping a request in `as`:

```json
{"cmd": "as", "identity": "slack:T1/U1", "request": {"cmd": "steer", "box": "demo", "text": "go"}}
```

The controller never reads its own working directory. The CLI makes a
relative `cp` destination or `env` repo absolute before it sends the request. It wraps a `start`, `start-needs` or `env` that names no repo in
`in`, with its own directory:

```json
{"cmd": "in", "dir": "/home/me/app", "request": {"cmd": "start", "name": "demo"}}
```

Only the operator may send `in`. A `start` with no repo and no `in` gets
`no repo`.

### Who may act on a Box

A Box acts for the user who started it. Only that user and the local operator
may act on the Box, for example to run, steer, show, close, sleep or wake it,
or read its artifacts. Only the operator may copy files out of it, because `cp`
writes on the controller host. A front gets `cp to the host is operator-only`.
The local operator is a peer on the
controller socket with the controller's own uid. A front socket is never the
operator, whatever identity it asserts. A Box with no user is the operator's
alone.

Anyone else gets `not your Box`. A Box that does not exist gets the same reply,
so Box ids cannot be probed. Box names are global, so a `start` can tell a name
that an open Box uses from a free one. A list, such as `moor show` with no Box
or `sessions`, shows only the caller's Boxes. `gate` reads only the recorded
runs of those Boxes. A warning in a reply never names a Box that the caller
may not see. A watch carries only the events of the caller's Boxes. A watch
frame may assert an `identity`, checked like a request's. A front's watch
carries the Boxes of the user it asserts. A watch on a front socket that
asserts no identity, or one the socket may not assert, carries no events.
A watch frame may also carry a `parent` Session id, which narrows the stream to
events of that Session's child Sessions. It never widens what the viewer may see.

A host Session belongs to the user who opens it. That user and the operator
may list, show, update, end and bind it, and start Boxes under it. Anyone else
gets `not your Box`, as for a Session that does not exist, and their lists
leave it out. A host Session the operator opens is the operator's only. An
identity with no user gets `log in first` when it opens one.

Only the operator names a host Session's `workdir`. Anyone else who names one is
refused. Their Session gets a directory of its own under the controller's
state, `users/<user>/sessions/<session>`, created owner-only. A `key` on
`open-host-session` is the caller's own opaque name for the Session, at most
256 bytes. While the caller has a live host Session with that key, opening
again returns it. A key never matches another user's Session.

`env` reads the controller host's configs, so only the operator may send it.

## Secrets a Box uses

Each Proxy rule names the secret it injects. A Box's Proxy injects the
secrets of the Box's user only. The Proxy reads each secret from the vault on
every request and keeps no copy, so a changed secret counts from the next
request. After `moor user secret rm`, the next request that needs the secret
fails, and the Box records `proxy_credential_failed`.

The secret names come from the rules that apply to the Box. A Box on GitHub
needs `claude` and `github`, and a rule you add under `proxy.rules` adds its
own name. `moor start` refuses, before it creates anything, when the user
lacks one of them. The refusal names every missing secret at once, in its
`value.missing`. A front can ask first with `start-needs`, which answers with
`needs` and `missing` for the caller and starts nothing:

```json
{"cmd": "start-needs", "identity": "slack:T1/U1", "repo": "acme/demo"}
```

`spec.json`, the file the controller writes for each Proxy, names the user and
the path of `vault.key`. It never holds a secret or the key itself. The Proxy
loads the key with the same checks as the controller and never creates one.

## Vault key

The controller encrypts each user secret in `moor.db` with
[age](https://age-encryption.org) to one X25519 key. The key is `vault.key`
in the controller config's directory. The controller creates it with mode
0600 on its first start.

The controller refuses to start, and leaves the file as it is, when
`vault.key` is a symlink, is not a regular file, belongs to another user, or
is open to group or others. The controller also refuses a config inside the
state directory, so the key never sits next to `moor.db`.

### Back up the vault

Back up `moor.db` and `vault.key`, and keep the two copies apart. Neither file
alone gives a secret back. If you lose `vault.key`, every stored secret is
lost, and each user must set them again.

The encryption protects secrets in a copy of `moor.db`, such as a backup, a
disk image, or a file attached to a bug report. It does not protect them from
a process that runs as the controller's user, because that process can read
`vault.key` too.
