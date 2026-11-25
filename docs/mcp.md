# MCP tools

The gate serves MCP at `https://HOST/mcp` over streamable HTTP. A client signs
in with OAuth, and every call acts as the person who signed in. A person sees
and changes only their own Boxes. `https://HOST/.well-known/oauth-protected-resource`
names the issuer and the scopes.

moor does not register clients. The client registers with the sign-in
provider, so the provider must allow the client's redirect URIs. A
`dcr: invalid redirect_uri` error comes from the provider's client or dynamic
registration settings, not from moor. The gate refuses a token whose `aud` is
not its `--mcp-resource-url`, so the provider must put that URL in the
token's audience.

## Scopes

| Scope | Allows |
| --- | --- |
| `moor:read` | `list_boxes`, `list_nodes`, `box_status`, `inspect_box`, `get_artifact`, `list_webhooks`, `list_workflows`, `get_workflow`, `list_workflow_runs`, `get_workflow_run`, `get_repo_config`, `list_events`, `GET /events`, artifact downloads, the Box resources, `subscriptions/listen` |
| `moor:write` | `start_box`, `ask`, `send_to_box`, `exec_in_box`, `close_box`, `retry_box`, `close_failed_boxes`, `add_webhook`, `remove_webhook`, `add_workflow`, `remove_workflow`, `set_workflow_enabled`, `answer_run`, `answer_gate`, `run_workflow`, `set_repo_config` |

## Tools

| Tool | Arguments |
| --- | --- |
| `list_boxes` | none |
| `list_nodes` | none |
| `start_box` | `repos`; optional `name`, `prompt`, `base`, `node`, `config`, `gates`, `wait` |
| `ask` | `repos`, `prompt` |
| `box_status` | `box` |
| `inspect_box` | `box`; optional `lines`, `said` |
| `get_artifact` | `box`, `name`; optional `version` |
| `send_to_box` | `box`, `text` |
| `exec_in_box` | `box`, `command`; optional `timeout_seconds` |
| `close_box` | `box`; optional `force` |
| `retry_box` | `box` |
| `close_failed_boxes` | optional `force` |
| `add_webhook` | `url`; optional `events`, `header` |
| `list_webhooks` | none |
| `remove_webhook` | `id` |
| `list_events` | optional `after`, `kinds`, `box`, `limit`, `wait_seconds` |
| `add_workflow` | `name`, `repos`, `prompt`. Optional: `cron` with `timezone` (none means by hand or on pull requests), `config`, `timeout_minutes`, `plan`, `gates` (in place of a plan; a gate's check is a command or `{run, says}`), `expected_revision`, `enabled` (false stores it disabled), `on_pull_request` (true also fires on the first repo's pull requests) |
| `list_workflows` | none. One compact row each; the prompt, plan, config, gates and run detail stay with `get_workflow` |
| `get_workflow` | `name` |
| `list_workflow_runs` | optional `after`, `limit`, `workflow` (one workflow's runs). Compact rows without history or result. Without `after`, the latest `limit` changes; `after` 0 reads from the oldest |
| `get_workflow_run` | `workflow`, `run` (the `id` of a listed row). The whole run with its history and result |
| `remove_workflow` | `name` |
| `set_workflow_enabled` | `name`, `enabled` |
| `answer_run` | `workflow`, `answer` (`yes` or `no`); optional `note`, `pull` (`OWNER/NAME#NUMBER`, which run when several wait) |
| `answer_gate` | `box`, `answer` (`yes` or `no`); optional `note` (told to the agent), `ask` (the `needs.id` answered) |
| `run_workflow` | `name`, with optional `expected_revision` and `pull` (`OWNER/NAME#NUMBER`) |
| `get_repo_config` | `repo` |
| `set_repo_config` | `repo`, `text`; optional `expected_revision` |
| `get_node_settings` | `name` |
| `set_node_settings` | `name`; optional `max_boxes`, `no_cap`, `labels`, `expected_revision` |

- `list_nodes` gives each node's name, scope, runtime, online state, Boxes,
  `max_boxes` when it caps them, and free memory. Its `start` field
  names the node a start with no `node` would use.
- `start_box` takes `repos` as a list of `owner/name`, first one first. The
  agent works in the first, and the rest are cloned beside it. A list of one
  adds your `box.also` beside it. It refuses before it creates
  anything when you lack a secret, and links the portal. It
  returns once the Box exists, with the fields `box_status` gives. With
  `wait: true`, it returns once setup and the agent launch finish. A start on
  a name that is open attaches to that Box.
- `ask` sends a question to your agent place, the request the portal's Tasks home sends. `repos` is a list of `owner/name`, the task's own first. It answers the place that took it (`box`, `claimed`), then follow it with `box_status`. A caller not on the controller's `agent_places` list is refused, and the refusal names the email moor holds for them.
- `send_to_box` answers a pending question with an option's name or its
  1-based number.
- `close_box` refuses a Box with commits that are not pushed or files that are
  not committed. `force: true` discards that work and also stops a setup that
  is running.
- `exec_in_box` runs shell text beside the agent, with no stdin and no
  terminal. It wakes an asleep Box. The timeout is 30 seconds by default and
  120 at most. It returns `exit`, `timed_out`, and the first 64 KiB of
  `stdout` and `stderr`. Each call records an `exec_ran` event.
- `retry_box` reruns setup on a Box in `setup_failed`. It returns once setup
  has restarted, while setup goes on. Follow it with `box_status`.
- `add_workflow` on an existing name keeps every optional field you leave out. That covers `cron` and `timezone`, `config`, `timeout_minutes`, `enabled`, `on_pull_request` and `gates`. To clear the cron, send `cron: null`. To clear the config, send `config: ""`. To clear the gates, send `gates: []`. The `plan` is the exception, so send the whole plan again. A new workflow is enabled, with no pull request trigger, no cron, no config and a 180 minute timeout.
- `run_workflow` returns once the run is recorded and its Box start
  has begun, while the Box sets up. A start the controller refuses after that
  shows as a refused run in `list_workflow_runs`.
- No tool waits for setup by default. Only `start_box` with `wait: true` does.
- `add_webhook` returns the webhook's `id` and a `setup_url`, never a secret.

### `box_status`

| Field | Meaning |
| --- | --- |
| `status` | One line, such as `ready · waiting on CI` or `ready · stuck: stalled`. |
| `agent_said` | `{text, at}`, the agent's newest message. |
| `stuck` | `{kind, since, detail}`, or `null`. |
| `artifacts` | The latest version of each published artifact. |
| `reports` | The latest version of each Report. |
| `events` | The last five events. |

`stuck.kind` is `stalled`, `egress_refused`, `egress_aborted`,
`credential_withheld`, `proxy_credential_failed`, `agent_rate_limited`,
`agent_unreachable`, `agent_link_lost`, or `death_capped`. `stalled` means the agent
reported busy and wrote nothing for ten minutes. The next sign of progress clears it.
`death_capped` means the Box's places died, so moor did not resume it on its own.
Waking the Box or messaging it clears it.
The `egress_aborted` detail asks whether the Box clock is behind, because a
guest clock behind the host makes the Proxy's certificates not yet valid.
It clears once the Proxy allows a request to that host again.

### `inspect_box`

Returns `pane`, the last `lines` lines of the agent's terminal (40 by default,
200 at most), with a `status` of `live`, `exited`, or `gone`. Also returns
`said`, the agent's newest messages (5 by default, 20 at most), `agent`, and
`stuck`. It changes nothing.

### `list_events`

Reads your events from the log, from an offset you keep.

| Argument | Default | Meaning |
| --- | --- | --- |
| `after` | 0 | Return events with a greater offset. |
| `kinds`, `box` | all | Only these kinds, only this Box. |
| `limit` | 50 | At most 100. |
| `wait_seconds` | 0 | When nothing is past `after`, wait up to this long. At most 30. |

It returns `{events, next, caught_up}`. [Events](events.md#event-kinds) lists
the kinds, such as `closed` and `close_refused`. Pass `next` as `after` in the
next call. The same arguments return the same events. Offsets increase with gaps,
because other users' events share the log.

## HTTP endpoints

The same bearer token works on these paths.

| Path | Scope | Returns |
| --- | --- | --- |
| `GET /events` | `moor:read` | The `list_events` result. Takes `after`, `kind` (repeatable), `box`, `limit`, and `wait_seconds`. |
| `GET /boxes/{box}/artifacts/{name}[/{version}]` | `moor:read` | The artifact's bytes, with its `Content-Type` and an `ETag` of its sha256. |

| Status | When |
| --- | --- |
| 401, 403 | No valid token, or a token without the scope. |
| 404 | No such Box, artifact name, or version. |
| 409 | The stored bytes do not match their sha256. |
| 410 | The artifact is recorded but its bytes are gone. |

## Box resources

Each open Box is the resource `moor://box/<name>`. `resources/read` returns the
`box_status` JSON.

A client on MCP 2026-07-28 can call `subscriptions/listen` once and keep the
stream open. It needs no `initialize`. Each POST carries
`MCP-Protocol-Version: 2026-07-28`, `Mcp-Method`, and, for `resources/read`,
`Mcp-Name`. `params._meta` carries the protocol version, `clientInfo`, and
`clientCapabilities`.

```json
{"jsonrpc": "2.0", "id": 7, "method": "subscriptions/listen", "params": {
  "_meta": {
    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
    "io.modelcontextprotocol/clientInfo": {"name": "me", "version": "1"},
    "io.modelcontextprotocol/clientCapabilities": {}
  },
  "notifications": {
    "resourcesListChanged": true,
    "resourceSubscriptions": ["moor://box/app/fix-login"]
  }}}
```

A state change of a listed Box sends `notifications/resources/updated` with its
URI. A Box that starts, closes, or is deleted sends
`notifications/resources/list_changed`. Read the resource to see what changed.
Events while no stream is open are not replayed. The gate sends an SSE comment
every 15 s.
