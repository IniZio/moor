# Set up the Slack front

`moor-slack` is a separate daemon that answers Slack over Socket Mode and
sends requests to the moor controller over its socket. A mention in an
channel the bot is in opens a thread with a coordinator agent, and the coordinator
starts and steers Boxes for the person who started the thread.

## Before you start

You need:

- a running controller (`moor controller serve`) with a `slack` entry under
  `fronts` that lists `proxy_rules: [claude]`, and for each person a `github`
  secret that can read the channel's repos
- Claude Code 2.1.304 or newer on the controller host
- a Slack workspace where you can create an app

## Create the Slack app

1. Create a Slack app and turn on Socket Mode.
2. Create an app-level token with the `connections:write` scope.
3. Give the bot token these scopes: `app_mentions:read`, `channels:history`,
   `chat:write`, `commands`, `files:read`, `files:write`, and `reactions:write`.
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
   ```

   There is no channel list. Invite the bot to a channel with `/invite @moor`
   and it answers there. Slack's own channel membership is the only gate on
   where the front answers. Anyone in a channel the bot is in can mention it
   and run `/moor login`. A config that still has a `channels` key is
   refused.

## Run the front

Start `moor-slack` with the controller's `slack` front socket and the config:

```sh
moor-slack --socket ~/.local/state/moor/fronts/slack.sock \
  --config ~/.config/moor-slack/config.yaml
```

A `command` under `fronts.slack` lets the controller start the front itself.
It then sets `MOOR_FRONT_SOCKET`, and `--socket` is not needed. Coordinator
turns need this: only a front the controller runs gets the loopback port of
the front Proxy in `MOOR_FRONT_PROXY_PORT`, and its CA in
`MOOR_FRONT_PROXY_CA_PEM`, which the front writes to `proxy-ca.pem` in its
state directory.

The front reads the workspace's team id from Slack once at startup, with
`auth.test`, and stops with an error if Slack does not answer. It acts for each
person as `slack:<team>/<user>`: every request it sends asserts that identity,
and it opens one watch per person who owns a thread. The controller's own
socket speaks only for the local operator, so it refuses the front.

The front logs the Claude Code version it found. If it finds no `claude`, or
one older than 2.1.304, the coordinator is unavailable, and the front posts one line in each
thread that says why.

## Start work from Slack

1. Each person links their own accounts. Run `/moor login` in any channel. The
   form has two optional fields. Paste the token that `claude setup-token`
   prints on your own machine into the Claude field and the output of
   `gh auth token` into the GitHub field. A blank field keeps the account you
   already linked, and a form with both fields blank is refused. Each filled
   token is checked on its own, and the form reports each result, so one
   refused token does not stop the other. `/moor login claude` and
   `/moor login github` open a form with that one field. The first link makes
   you a moor user, and each token goes into the controller's vault under your
   name.
2. In a channel the bot is in, add a GitHub repo: `/moor repo add OWNER/NAME`.
3. Mention the bot at the top level of the channel and describe the work. The
   coordinator replies in a thread and starts Boxes when you ask for work.

## Command line

| Flag | Environment variable | Default | Meaning |
| --- | --- | --- | --- |
| `--socket PATH` | `MOOR_SLACK_SOCKET`, else `MOOR_FRONT_SOCKET` | required | The controller's `slack` front socket. |
| `--config PATH` | `MOOR_SLACK_CONFIG` | required | The front's config file. |
| `--state-dir DIR` | `MOOR_SLACK_STATE_DIR` | `$MOOR_FRONT_STATE_DIR`, set by the controller that runs it, else `$XDG_STATE_HOME/moor-slack`, else `$HOME/.local/state/moor-slack` | The front's own state, including `slack.db`. |

The front refuses a `slack.db` that another build wrote at a different
schema. Move it aside and start again with a fresh one.

## How a thread behaves

- The bot answers in any channel it has been invited to: a mention opens a
  thread, replies in the thread reach it, and `/moor repo` commands work in
  that channel.
- Only the person who started the thread directs the coordinator. Anyone else
  who writes in the thread gets one line that says so.
- Each message from the starter becomes one turn. A turn is one `claude -p`
  process that runs with the starter's own token. Messages that arrive during a turn
  go into the next turn together. The front stops a turn that runs past 10
  minutes. Up to 4 turns run at once.
- The coordinator's only tools are `list_boxes`, `start_box`, `steer_box`,
  `close_box`, `answer_box`, `show_box`, `interrupt_box`, `sleep_box`,
  `wake_box`, and `restart_agent`. They act only on the thread's own Boxes. A
  tool that changes a Box works only in a turn that holds a message from the
  person who asked for that Box. `show_box` only reads (the agent's state,
  the question or first-run screen it waits on, and the last lines of its
  pane), so anyone in the thread may ask for it. `restart_agent` ends the
  agent's session and resumes it, or starts the Box's goal over when there is
  no session to resume.
- Each Box's Session is a child Session of the thread's host Session. The
  Box's name is `slack-<thread ts>-<n>`, with each `.` in the thread
  timestamp replaced by `-`.
- While a turn runs, the front puts an :hourglass_flowing_sand: reaction on
  the message that started the turn (the latest one when a turn holds several),
  and takes it off when it posts the reply or the line that says why there is
  none. After a restart it takes off the reaction
  of a turn that will not finish. Without the `reactions:write` scope the
  thread shows no reaction and the replies still post; the front logs the
  missing scope once.
- Each Box has one status message in the thread. The front edits it as the
  Box's state changes, at most once every 2 seconds. It names the phase
  (starting, setting up, working, waiting on a question, asleep, closing) and,
  in whole minutes, how long the Box has been in it. While the Box works,
  a line `now: <what the agent is doing>` in a code span shows its latest
  activity, such as `Bash: cargo test`; it goes when the turn ends. A working
  Box that has shown no new activity for 10 minutes gets a line that says it
  may be stuck; the time counts from its last activity line, or from the
  turn's start when it has shown none. So does a Box whose agent has not started a session 3 minutes after
  the launch began (setup time does not count), a Box stopped at a first-run
  screen for 3 minutes, and a Box waiting on an unanswered question for 3
  minutes. Each line says what the Box is stuck on and ends with how to go on:
  ask the coordinator to restart the agent, or answer the question. The elapsed time changes the text once a minute, so a long phase adds
  one edit a minute. While the Box is working or waiting on a question, the
  message carries a Stop button; only the person who asked for the Box can use
  it, and it interrupts the agent. The button goes when the Box closes.
- A turn that has run `long_turn_minutes` (20 by default) gets a new message
  in the Box's thread, so the requester is notified: "`<box>` has been working
  for 20m. Now: `<what it is doing>`. Reply here to redirect it, or ask me to
  stop it." The "Now" part is left out while the agent has shown no activity.
  The same turn gets one more every further `long_turn_minutes`, and none once
  it ends. A restart posts none twice.
- The thread gets one plain line when a Box's agent could not start, did not
  start within `box.launch_timeout`, was started again on its own after it
  ended, or was not started again because it ended 3 times in 10 minutes. It
  gets one when the sleep timer puts the Box to sleep. A sleep or a restart
  you asked for gets no line. Each line ends with what to reply: ask the
  coordinator to restart the agent, or reply to wake the Box. Each also reaches
  the coordinator as a notice with the thread's next message.
- When a Box's agent session ends without a clean exit (a non-zero or unknown
  exit code), the thread gets one plain line in place of "finished its turn":
  the agent stopped unexpectedly, with the exit code, plus ", and its
  transcript could not be saved" when the controller could not save it. The
  Box's work is still in the Box. The line ends with what to reply: ask the
  coordinator to restart the agent. The coordinator gets the same line, so it
  does not say the turn finished. A clean exit keeps the finished line.
- When a Box's agent asks a question, the front posts it with one button per
  option, up to 25. Only the person who asked for the Box can answer.
- Each file version a Box publishes is uploaded into the thread, normally once.
  A crash between Slack sharing the file and the front recording it can upload
  that version a second time. Without `files:read`, the front cannot check a lost upload completion and may upload that file again; it logs this once. Without the `files:write` scope, the thread shows
  the file's name and size instead. When an upload fails, the front posts the
  reason (if that post is refused, it posts it on a later try) and tries again
  with a growing wait of 2 seconds up to 64. A retry waits on its own and holds
  back no other thread. 10 minutes after the first failure the front stops and
  posts that it gave up, with the reason, when Slack accepts that post. That
  version is never tried again, not even after a restart.
- After `idle_minutes` with no turn and no waiting message, the front ends the
  thread's host Session. The front keeps the host Session while a Box is starting or
  closing, while an agent works or waits on a question, or while the
  controller cannot report a Box's state. Ending the host Session leaves
  every Box as it is. The starter's next message opens a new one.

## Where the front keeps secrets

- `/moor login` stores no token in the state directory. After the account
  accepts the token, the front sends the controller `enroll` and then
  `secret-set`, each as the person who pasted it, and the controller keeps the
  secret in its vault under the name `claude` or `github`.
- The `/moor login claude` check gets the pasted token on a pipe at file
  descriptor 3, never in its arguments, its environment, `slack.db`, or the
  log. The front stores the token only when the check passes.
- A coordinator turn never holds a token. Before the turn starts, the front
  sends the controller `proxy-lease` as the starter, and the controller grants
  a lease of the front Proxy for the starter's moor user. The turn's
  `HTTPS_PROXY` carries the lease as the proxy password, and the Proxy puts the
  starter's `claude` secret on each request to `api.anthropic.com`. The front
  sends `proxy-release` when the turn ends. A lease ends by itself after 15
  minutes and when the controller restarts.
- The front needs `proxy_rules: [claude]` in its controller `fronts` entry; see
  [config.md](config.md#fronts). Without it every turn says the controller gave
  this front no Proxy.
- A starter with no `claude` secret is asked to run `/moor login`. When the
  Proxy is down at the start of a turn, or when a turn fails and the controller
  says the Proxy is down, the thread says that the Proxy is not running and
  that moor is restarting it.
- A turn runs in `/` with a cleared environment and no built-in tools. It gets
  its own Claude config directory per Slack user, and
  `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1`. A turn refuses to start when `/` holds a
  `CLAUDE.md`, an `AGENTS.md`, or a `.claude/rules/` file, or when
  `/etc/claude-code` would load instructions.
- The tool server, `moor-slack mcp`, reads the thread's rows in `slack.db` and
  asks the controller. It never sees a Slack or Claude token. Claude Code
  passes it the turn's `HTTPS_PROXY`, so it sees the turn's lease.

## Config reference

The front rejects unknown keys.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `app_token_file` | path | required | The file that holds the `xapp-` token. It must be mode 0600 and not empty. `~` is allowed. |
| `bot_token_file` | path | required | The file that holds the `xoxb-` token. It must be mode 0600 and not empty. `~` is allowed. |
| `api_base` | URL | `https://slack.com/api` | The Slack Web API base. It must start with `https://`, `http://127.0.0.1:`, or `http://localhost:`. A loopback base works only when you run moor-slack yourself: a front the controller runs on Linux has a loopback of its own. |
| `github_api_base` | URL | `https://api.github.com` | The GitHub API base where a pasted `gh auth token` is checked with `GET /user`. The same URL rules as `api_base`. |
| `long_turn_minutes` | integer, at least 1 | 20 | Minutes one Box turn may run before its thread gets a line saying so, and again after each further so many minutes of the same turn. |
| `coordinator.binary` | absolute path | `claude` on `PATH` | The Claude Code binary. |
| `coordinator.agent` | `claude` | `claude` | The coordinator agent. |
| `coordinator.model` | string | `sonnet` | The model for each turn. |
| `coordinator.context_kib` | integer, at least 1 | 32 | The context budget for one turn, in KiB. |
| `coordinator.max_open_boxes` | integer, at least 1 | 1 | The most Boxes that one thread has open at once. |
| `coordinator.idle_minutes` | integer, at least 1 | 30 | Quiet minutes before a thread's host Session ends. |
| `roleplay.users` | list of Slack user ids, at least one | off | For test automation only. A post the bot makes with its own token and with message metadata `{"event_type": "moor_roleplay", "event_payload": {"user": "U…"}}` for a listed user counts as that user's: a top-level post naming the bot starts a thread, a thread post is a reply. Anyone holding the bot token can then act as the listed users. Each such message is logged with its channel, ts and user. |

## Slash commands

| Command | Who | Where | What it does |
| --- | --- | --- | --- |
| `/moor repo` | anyone | any channel the bot is in | Lists the channel's repos and marks the default. |
| `/moor repo add OWNER/NAME` | anyone | any channel the bot is in | Adds a GitHub repo. A name that is not `OWNER/NAME` is refused. A start of a repo the person's GitHub account cannot read is refused by the controller. |
| `/moor repo remove NAME` | anyone | any channel the bot is in | Removes a repo. |
| `/moor repo default NAME` | anyone | any channel the bot is in | Makes `NAME` the channel's default repo. |
| `/moor login [claude\|github]` | anyone | any channel | Opens a form for the tokens: both optional fields with no word, one field with `claude` or `github`. The front checks a Claude token with one test call to Claude on `haiku`, and a GitHub token with `GET /user`, which must answer 200. It then stores the token in the controller's vault as you. A 401 or 403 stores nothing. |
| `/moor logout [claude\|github]` | anyone | any channel | Deletes that secret from the vault and tells you whether one was there. Claude is the default. |
