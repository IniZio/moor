# Configuration reference

moor reads three YAML files. moor rejects unknown keys in each file. An empty
file means every key takes its default.

| File | Who writes it | Where it is |
| --- | --- | --- |
| [User config](#user-config) | You | `$MOOR_CONFIG`, else `$XDG_CONFIG_HOME/moor/config.yaml`, else `$HOME/.config/moor/config.yaml` |
| [`.moor.yaml`](#mooryaml) | The repo | The repo's base commit, read through GitHub when the Box starts |
| [Controller config](#controller-config) | Whoever runs the controller | The path given to `moor controller serve --config` |

## User config

### `box`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `box.provider` | string | `local-container` | The crabbox provider for new Boxes. A repo cannot set it. |
| `box.size` | `small`, `medium`, `large`, or `xlarge` | `small` | The Box size. A `.moor.yaml` value wins over this one. |
| `box.max_boxes` | integer, at least 1 | none | The most live Boxes at once. |
| `box.sleep_after` | duration such as `30s`, `45m`, `2h`, or `1d` | `30m` | How long a Box stays idle before moor puts it to sleep. |

### `drivers`

`drivers.<provider>` holds settings for one provider. See
[Box providers](providers.md) for how they work.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `max_boxes` | integer, at least 1 | none | The most live Boxes on this provider. It wins over `box.max_boxes`. |
| `lookup_timeout_secs` | integer, at least 1 | 5, or 20 for `external` | How long moor waits for crabbox to look up a Box's ssh target. |
| `strip_env` | list of variable names | empty | Variables that the provider puts in the Box, which moor removes from every command it runs there. |

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

`args`, `env` values, `url`, and `headers` values can hold `${NAME}`, which
names the Box user's secret `NAME`. `NAME` may not be a secret that a Proxy
rule injects.

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
| `setup` | list of shell commands | Commands that run on each new machine. |
| `teardown` | list of shell commands | Commands that run when the Box closes. |
| `env` | map | Plain environment variables for the Box. `PATH` is reserved. moor does not pass names that look like credentials, or names that moor sets, to the agent. |
| `compose.project_prefix` | string | A prefix for compose project names. |
| `box.size` | `small`, `medium`, `large`, or `xlarge` | The Box size. It wins over the user's `box.size`. |
| `box_credentials` | list of names | Secrets that the Box's user must hold for a `start`. moor does not put them into the Box. |

moor reads `.moor.yaml` from GitHub at the Box's base commit when the Box
starts, and keeps that copy for the Box's life. Setup, the agent, `moor run`
and the teardown all use it, so a change the agent makes to the file in the
Box never changes what moor runs. A changed `.moor.yaml` takes effect in the
next Box after it is pushed.

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
  slack: {}
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `command` | list of strings, not empty | none | The program and arguments the controller runs as this front. Without it, start the front yourself. |

For each entry the controller listens on `<state dir>/fronts/<namespace>.sock`,
mode 0600 in a 0700 directory. Under systemd, the controller takes the socket
that systemd passes as `front-<namespace>` instead of binding it. A namespace matches `[a-z][a-z0-9-]*` and is
not `local`.

When an entry has a `command`, the controller starts it at startup. The front
gets an `O_PATH` file descriptor to its own socket as fd 3, and
`MOOR_FRONT_SOCKET=/proc/self/fd/3` names it, so the front connects to that
path. The front's stdin is empty and its stdout is the controller's. Each
line it writes to stderr goes to the controller log as `front <namespace>:
<line>`.

Each front runs under `moor front-shim`, in its own user, mount and PID
namespaces, with the controller's uid and gid and no capabilities. There the
state dir is an empty read-only tmpfs, the controller socket and the vault key
read as `/dev/null`, and `/proc` shows only the front's own processes, so fd 3
is the front's only way to the controller. The shim is pid 1 of that
namespace: it reaps orphans, passes SIGTERM and SIGINT to the front, and exits
with the front's status. If the controller's shim dies, the kernel kills every
process in the namespace. Fronts need unprivileged user namespaces. When the
host forbids them (`kernel.unprivileged_userns_clone=0`,
`user.max_user_namespaces=0`, or `kernel.apparmor_restrict_unprivileged_userns=1`),
the controller logs `fronts not started` with the reason, starts none, and
keeps serving.

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
{"cmd": "as", "identity": "slack:T1:U1", "request": {"cmd": "steer", "box": "demo", "text": "go"}}
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

Some requests read or change the controller host, so only the operator may
send them:

- `env`.
- Host Sessions. Anyone else gets `not your Box` for a host Session, as for a
  Session that does not exist, and a list leaves host Sessions out.

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
{"cmd": "start-needs", "identity": "slack:T1:U1", "repo": "acme/demo"}
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
