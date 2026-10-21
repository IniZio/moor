# Configuration reference

moor reads three YAML files. moor rejects unknown keys in each file. An empty
file means every key takes its default.

| File | Who writes it | Where it is |
| --- | --- | --- |
| [User config](#user-config) | You | `$MOOR_CONFIG`, else `$XDG_CONFIG_HOME/moor/config.yaml`, else `$HOME/.config/moor/config.yaml` |
| [`.moor.yaml`](#mooryaml) | The repo | The Box worktree, else the repo's main checkout |
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

### `credentials`

`credentials.<NAME>` says where moor reads the credential `NAME`. Give exactly
one source.

| Key | Type | Meaning |
| --- | --- | --- |
| `cmd` | list of strings | A command whose output is the value. |
| `file` | path | A file that holds the value. |
| `env` | variable name | A host environment variable that holds the value. |
| `pass_env` | list of variable names | Extra host variables that `cmd` may read. Only with `cmd`. |

The Proxy no longer reads these sources. A Box's Proxy injects the secrets
of the Box's user. See [Secrets a Box uses](#secrets-a-box-uses).

### `box_credentials`

A list of credential names whose values moor puts in the Box as environment
variables. A `.moor.yaml` can list names too. The values always come from
`credentials`. moor refuses `GH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`, and any
name that starts with `ANTHROPIC_`, because the Proxy serves those.

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
moor replaces with the credential `NAME`.

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
| `requires_you` | boolean | If true, moor drops the rule unless the GitHub token is your own. |

The default rules are `claude` for `api.anthropic.com`, which injects the
`claude` secret, and `github-api`, `github-git`, and `github-uploads` for
`api.github.com`, `github.com`, and `uploads.github.com`, which inject the
`github` secret. They are in
[defaults.yaml](../crates/moor-core/src/config/proxy/defaults.yaml). A user
rule that extends a default cannot set `hosts` or `credential`. Its `allow`
entries come after the default's, and its `box_env` values win.

`proxy.direct` lists hosts that the Box reaches without the Proxy. See
[Direct hosts](providers.md#direct-hosts).

### `github`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `github.identity` | `you`, `bot`, or `auto` | `auto` | Which GitHub identity acts. `auto` acts as `you`. |
| `github.you_token_cmd` | list of strings | `[gh, auth, token]` | The command that prints your GitHub token. |
| `github.pass_env` | list of variable names | empty | Host variables passed to `you_token_cmd`. |
| `github.app.id` | integer | none | The GitHub App id. |
| `github.app.key_path` | path | none | The App's private key file. |
| `github.app.install` | map | empty | Installation settings for the App. |

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
| `box_credentials` | list of names | Credentials from the user's `credentials` to put in the Box. |
| `proxy.direct` | list of host names | Hosts that the Box reaches without the Proxy. A repo cannot add or change a Proxy rule. |

If a Box's `.moor.yaml` differs from the copy in its base commit, moor refuses
to run setup again, start the agent, or run `moor run`. Review the change, then
repeat the command with `--trust-moor-yaml`.

## Controller config

`moor controller serve --config FILE` reads this file.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `socket` | path | `<state dir>/controller.sock` | The controller socket. `~` is allowed. |
| `state_dir` | path | the same state directory as the CLI | The state directory. `~` is allowed. |
| `settle_every_secs` | integer, above 0 | 5 | Seconds between passes that update each open Box's agent state. |
| `repos` | map | empty | Repos the controller can clone on its own host. |
| `fronts` | map | empty | Fronts that each get their own socket. |

The controller refuses a config that has a `web` or `slack` key.

### `repos`

`repos.<name>` names a repo for `moor start --repo`. The name matches
`[A-Za-z0-9][A-Za-z0-9._-]*`.

| Key | Type | Meaning |
| --- | --- | --- |
| `url` | string | The clone URL, for the host's own `git clone`. It must not hold a password or token. |
| `default_branch` | string | The branch new Boxes start from. The default is the repo's default branch. |
| `moor_yaml.path` | path | A `.moor.yaml` on the controller host to use when the repo has none. moor resolves a relative path from the config file's directory. |
| `moor_yaml.inline` | string | The `.moor.yaml` text itself, used when the repo has none. |

### `fronts`

`fronts.<namespace>` gives one front its own socket. An entry has no settings:

```yaml
fronts:
  slack: {}
```

For each entry the controller listens on `<state dir>/fronts/<namespace>.sock`,
mode 0600 in a 0700 directory. A namespace matches `[a-z][a-z0-9-]*` and is
not `local`.

A moor user owns secrets, such as a Claude or GitHub token. An identity names
a person on one front, as `<namespace>:<rest>`, and a user can link several
identities with `moor user link-code`. A request on a front
socket must assert an identity in that front's namespace. On the controller
socket, the controller reads the peer's uid from the kernel and acts as `local:<uid>`. It
refuses any other identity.

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
{"cmd": "start-needs", "identity": "slack:T1:U1", "repo": "demo"}
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
