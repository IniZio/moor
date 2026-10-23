# Set up the Slack front

`moor-slack` is a separate daemon that answers Slack over Socket Mode and
sends requests to the moor controller over its socket. A mention in an
allowed channel opens a thread with a coordinator agent, and the coordinator
starts and steers Boxes for the person who started the thread.

## Before you start

You need:

- a running controller (`moor controller serve`), and for each person a
  `github` secret that can read the channel's repos
- Claude Code 2.1.304 or newer on the controller host
- a Slack workspace where you can create an app

## Create the Slack app

1. Create a Slack app and turn on Socket Mode.
2. Create an app-level token with the `connections:write` scope.
3. Give the bot token these scopes: `app_mentions:read`, `channels:history`,
   `chat:write`, `commands`, and `files:write`.
4. Add a `/moor` slash command.
5. Subscribe to the `app_mention` and `message.channels` bot events. Without
   `message.channels`, the front never sees a reply in a thread.
6. Turn on Interactivity. Socket Mode needs no request URL.
7. Install the app in the workspace, and invite the bot to each channel that
   it serves.

## Write the config

1. Put each token in its own file that only you can read:

   ```sh
   mkdir -p ~/.config/moor-slack
   install -m 600 /dev/null ~/.config/moor-slack/app-token
   install -m 600 /dev/null ~/.config/moor-slack/bot-token
   ```

   Write the `xapp-` token into `app-token` and the `xoxb-` token into
   `bot-token`.

2. Write `~/.config/moor-slack/config.yaml`:

   ```yaml
   app_token_file: ~/.config/moor-slack/app-token
   bot_token_file: ~/.config/moor-slack/bot-token
   channels: [C0123456789]
   users: [U0123456789]
   ```

   `channels` and `users` take Slack ids, not names.

## Run the front

Start `moor-slack` with the controller socket and the config:

```sh
moor-slack --socket ~/.local/state/moor/controller.sock \
  --config ~/.config/moor-slack/config.yaml
```

The front logs the Claude Code version it found. If it finds no `claude`, or
one older than 2.1.304, the coordinator is unavailable, and the front posts one line in each
thread that says why.

## Start work from Slack

1. Each person links their own Claude token. Run `/moor login` in any
   channel. Run `claude setup-token` on your own machine and paste the token
   it prints into the form.
2. In an allowed channel, add a GitHub repo: `/moor repo add OWNER/NAME`.
3. Mention the bot at the top level of the channel and describe the work. The
   coordinator replies in a thread and starts Boxes when you ask for work.

## Command line

| Flag | Environment variable | Default | Meaning |
| --- | --- | --- | --- |
| `--socket PATH` | `MOOR_SLACK_SOCKET` | required | The controller socket. |
| `--config PATH` | `MOOR_SLACK_CONFIG` | required | The front's config file. |
| `--state-dir DIR` | `MOOR_SLACK_STATE_DIR` | `$XDG_STATE_HOME/moor-slack`, else `$HOME/.local/state/moor-slack` | The front's own state, including `slack.db`. |

The front refuses a `slack.db` that another build wrote at a different
schema. Move it aside and start again with a fresh one.

## How a thread behaves

- Only the person who started the thread directs the coordinator. Anyone else
  who writes in the thread gets one line that says so.
- Each message from the starter becomes one turn. A turn is one `claude -p`
  process that runs with the starter's own token. Messages that arrive during a turn
  go into the next turn together. The front stops a turn that runs past 10
  minutes. Up to 4 turns run at once.
- The coordinator's only tools are `list_boxes`, `start_box`, `steer_box`,
  `close_box`, and `answer_box`. They act only on the thread's own Boxes. A
  tool that changes a Box works only in a turn that holds a message from the
  person who asked for that Box.
- Each Box's Session is a child Session of the thread's host Session. The
  Box's name is `slack-<thread ts>-<n>`, with each `.` in the thread
  timestamp replaced by `-`.
- Each Box has one status message in the thread. The front edits it as the
  Box's state changes, at most once every 2 seconds.
- When a Box's agent asks a question, the front posts it with one button per
  option, up to 25. Only the person who asked for the Box can answer.
- Each file version a Box publishes is uploaded into the thread once. Without
  the `files:write` scope, the thread shows the file's name and size instead.
  When an upload fails, the thread shows the reason once and the front tries
  the upload again every 2 seconds until it goes up.
- After `idle_minutes` with no turn and no waiting message, the front ends the
  thread's host Session. The front keeps the host Session while a Box is starting or
  closing, while an agent works or waits on a question, or while the
  controller cannot report a Box's state. Ending the host Session leaves
  every Box as it is. The starter's next message opens a new one.

## Where the front keeps secrets

- `/moor login` stores the token at `credentials/claude/<Slack user id>` in
  the state directory, mode 0600.
- A turn gets the token on a pipe at file descriptor 3, never in its
  arguments, its environment, `slack.db`, or the log.
- A turn runs in `/` with a cleared environment and no built-in tools. It gets
  its own Claude config directory per Slack user, and
  `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1`. A turn refuses to start when `/` holds a
  `CLAUDE.md`, an `AGENTS.md`, or a `.claude/rules/` file, or when
  `/etc/claude-code` would load instructions.
- The tool server, `moor-slack mcp`, reads the thread's rows in `slack.db` and
  asks the controller. It never sees a Slack or Claude token.

## Config reference

The front rejects unknown keys.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `app_token_file` | path | required | The file that holds the `xapp-` token. It must be mode 0600 and not empty. `~` is allowed. |
| `bot_token_file` | path | required | The file that holds the `xoxb-` token. It must be mode 0600 and not empty. `~` is allowed. |
| `channels` | list of channel ids | required, at least one | Channels that may start Boxes and use `/moor repo`. |
| `users` | list of user ids | required, at least one | People who may start Boxes, change a channel's repos, and run `/moor login`. |
| `api_base` | URL | `https://slack.com/api` | The Slack Web API base. It must start with `https://`, `http://127.0.0.1:`, or `http://localhost:`. |
| `coordinator.binary` | absolute path | `claude` on `PATH` | The Claude Code binary. |
| `coordinator.agent` | `claude` | `claude` | The coordinator agent. |
| `coordinator.model` | string | `sonnet` | The model for each turn. |
| `coordinator.context_kib` | integer, at least 1 | 32 | The context budget for one turn, in KiB. |
| `coordinator.max_open_boxes` | integer, at least 1 | 1 | The most Boxes that one thread has open at once. |
| `coordinator.idle_minutes` | integer, at least 1 | 30 | Quiet minutes before a thread's host Session ends. |

## Slash commands

| Command | Who | Where | What it does |
| --- | --- | --- | --- |
| `/moor repo` | `users` | `channels` | Lists the channel's repos and marks the default. |
| `/moor repo add OWNER/NAME` | `users` | `channels` | Adds a GitHub repo. A name that is not `OWNER/NAME` is refused. A start of a repo the person's GitHub account cannot read is refused by the controller. |
| `/moor repo remove NAME` | `users` | `channels` | Removes a repo. |
| `/moor repo default NAME` | `users` | `channels` | Makes `NAME` the channel's default repo. |
| `/moor login [AGENT]` | `users` | any channel | Stores your Claude token after one test call to Claude on `haiku`. |
| `/moor logout [AGENT]` | anyone | any channel | Deletes your stored token. |
