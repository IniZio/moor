# Set up the Slack front

`moor-slack` is a separate daemon. It answers Slack over Socket Mode and sends
requests to the moor controller over the controller's socket. A mention in a
channel the bot is in opens a thread with a coordinator agent. The coordinator
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
   `chat:write`, `commands`, `files:read`, `files:write`, `reactions:write`,
   `users:read`, and `users:read.email`. The last two let the front read each
   person's Slack email.
4. Add a `/moor` slash command.
5. Subscribe to the `app_mention` and `message.channels` bot events. Without
   `message.channels`, the front never sees a reply in a thread.
6. Turn on Interactivity. Socket Mode needs no request URL.
7. Install the app in the workspace, and invite the bot to each channel that
   it serves.

The front gives the controller each person's Slack profile email when it sets
that person up. The same email in Slack and in the sign-in provider is one moor
user, with the same secrets and machines in both. A different email in each
makes two users. Without the two scopes, or for a profile with no email, the
front sets the person up without one and logs the missing scope.

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

   The config has no channel list. To let the bot answer in a channel, invite
   it with `/invite @moor`. Slack's channel membership is the only gate on where
   the front answers. Anyone in a channel the bot is in can mention it and run
   `/moor login`. The front refuses a config that has a `channels` key.

## Run the front

Start `moor-slack` with the controller's `slack` front socket and the config:

```sh
moor-slack --socket ~/.local/state/moor/fronts/slack/front \
  --config ~/.config/moor-slack/config.yaml
```

If you set a `command` under `fronts.slack`, the controller starts the front
itself. It then sets `MOOR_FRONT_SOCKET`, so you do not need `--socket`.
Coordinator turns need a front that the controller runs. Only such a front
gets the loopback port of the front Proxy in `MOOR_FRONT_PROXY_PORT` and the
Proxy's CA in `MOOR_FRONT_PROXY_CA_PEM`. The front writes the CA to
`proxy-ca.pem` in its state directory.

At startup the front reads the workspace's team id from Slack once, with
`auth.test`. If Slack does not answer, the front stops with an error. The front
acts for each person as `slack:<team>/<user>`. Every request it sends asserts
that identity, and it opens one watch for each person who owns a thread. The
controller's own socket speaks only for the local operator, so it refuses the
front.

The front logs the Claude Code version it found. If it finds no `claude`, or
one older than 2.1.304, the coordinator is unavailable. The front then posts one
line in each thread that says why.

## Start work from Slack

1. Each person links their own accounts. Run `/moor login` in any channel. The
   form has two optional fields. Paste the token that `claude setup-token`
   prints on your own machine into the Claude field. Paste the output of
   `gh auth token` into the GitHub field. A blank field keeps the account you
   already linked. The front refuses a form with both fields blank. The front
   checks each filled token separately and reports each result, so one refused
   token does not stop the other. `/moor login claude` and `/moor login github`
   open a form with only that field. The first link makes you a moor user. Each
   token goes into the controller's vault under your name.
2. In a channel the bot is in, add a GitHub repo with
   `/moor repo add OWNER/NAME`.
3. Mention the bot at the top level of the channel and describe the work. The
   coordinator replies in a thread and starts Boxes when you ask for work.

## Command line

| Flag | Environment variable | Default | Meaning |
| --- | --- | --- | --- |
| `--socket PATH` | `MOOR_SLACK_SOCKET`, else `MOOR_FRONT_SOCKET` | required | The controller's `slack` front socket. |
| `--config PATH` | `MOOR_SLACK_CONFIG` | required | The front's config file. |
| `--state-dir DIR` | `MOOR_SLACK_STATE_DIR` | `$MOOR_FRONT_STATE_DIR`, set by the controller that runs it, else `$XDG_STATE_HOME/moor-slack`, else `$HOME/.local/state/moor-slack` | The front's own state, including `slack.db`. |

The front refuses a `slack.db` that another build wrote at a different schema.
Move the file aside and start again with a fresh one.

## How a thread behaves

- The bot answers in any channel it is invited to. A mention opens a thread,
  replies in the thread reach the bot, and `/moor repo` commands work in that
  channel.
- Only the person who started the thread directs the coordinator. Anyone else
  who writes in the thread gets one line that says so.
- Each message from the starter becomes one turn. A turn is one `claude -p`
  process that runs with the starter's own token. Messages that arrive during a
  turn go into the next turn together. The front stops a turn that runs past 10
  minutes. Up to 4 turns run at once.
- The coordinator's only tools are `list_boxes`, `start_box`, `steer_box`,
  `close_box`, `answer_box`, `show_box`, `interrupt_box`, `sleep_box`,
  `wake_box`, and `restart_agent`. They act only on the thread's own Boxes. A
  tool that changes a Box works only in a turn that holds a message from the
  person who asked for that Box. `show_box` only reads, so anyone in the thread
  can ask for it. It shows the agent's state, the question or first-run screen
  the agent waits on, and the last lines of its pane. `restart_agent` ends the
  agent's session and resumes it. If there is no session to resume, it starts
  the Box's prompt over.
- Each Box's Session is a child Session of the thread's host Session. The
  Box's name is `slack-<thread ts>-<n>`, with each `.` in the thread
  timestamp replaced by `-`.
- While a turn runs, the front puts an :hourglass_flowing_sand: reaction on the
  message that started the turn. When a turn holds several messages, the
  reaction goes on the latest one. The front takes the reaction off when it
  posts the reply, or the line that says why there is no reply. After a
  restart, the front takes off the reaction of a turn that will not finish.
  Without the `reactions:write` scope, the thread shows no reaction and the
  replies still post. The front logs the missing scope once.
- Each Box has one status message in the thread. The front edits it as the
  Box's state changes, at most once every 2 seconds. The message names the
  phase and how many whole minutes the Box has been in it. The phases are
  starting, setting up, working, waiting on a question, asleep, and closing.
- While the Box works, a line `now: <what the agent is doing>` in a code span
  shows its latest activity, such as `Bash: cargo test`. The line goes when the
  turn ends.
- The status message gets a line that says the Box may be stuck in these
  cases:
  - A working Box shows no new activity for 10 minutes. The time counts from
    its last activity line, or from the turn's start when it has shown none.
  - The agent has not started a session 3 minutes after the launch began.
    Setup time does not count.
  - The Box stops at a first-run screen for 3 minutes.
  - The Box waits on an unanswered question for 3 minutes.

  Each line says what the Box is stuck on and ends with what to do next: ask
  the coordinator to restart the agent, or answer the question.
- The elapsed time changes the text once a minute, so a long phase adds one
  edit a minute.
- While the Box is working or waiting on a question, the status message has a
  Stop button. Only the person who asked for the Box can use it, and it
  interrupts the agent. The button goes when the Box closes.
- When a turn has run `long_turn_minutes` (20 by default), the front posts a
  new message in the Box's thread, so Slack notifies the requester: "`<box>`
  has been working for 20m. Now: `<what it is doing>`. Reply here to redirect
  it, or ask me to stop it." The message leaves out the "Now" part while the
  agent has shown no activity. The same turn gets one more message after each
  further `long_turn_minutes`, and none after it ends. A restart does not post
  the same message twice.
- The thread gets one plain line when a Box's agent could not start, or did
  not start within `box.launch_timeout`. It also gets one when the agent
  started again by itself after it ended, or did not start again because it
  ended 3 times in 10 minutes. It gets one when the sleep timer puts the Box to
  sleep. A sleep or a restart that you asked for gets no line. Each line ends
  with what to reply: ask the coordinator to restart the agent, or reply to
  wake the Box. The coordinator also gets each line as a notice with the
  thread's next message.
- A Box's agent session can end without a clean exit, with a non-zero or
  unknown exit code. Then the thread gets one plain line in place of "finished
  its turn" as soon as the front sees the session end. The line says that the
  agent stopped unexpectedly and gives the exit code. When the controller could
  not save the transcript, the line adds ", and its transcript could not be
  saved". When the controller could not reach the Box to tell whether the agent
  still ran, the line says that the Box stopped answering and that its session
  was ended. The line ends with what to reply: ask the coordinator to restart
  the agent. The coordinator gets the same line, so it does not say that the
  turn finished. A clean exit keeps the "finished its turn" line. No line posts
  for a session that a later session replaced before the post, or for an agent
  whose link dropped and came back.
- When a Box's agent asks a question, the front posts it with one button per
  option, up to 25. Only the person who asked for the Box can answer.
- The front uploads each file version that a Box publishes into the thread
  once. A crash between Slack sharing the file and the front recording it can
  upload that version a second time. Without `files:read`, the front cannot
  check a lost upload completion and can upload that file again. It logs this
  once. Without the `files:write` scope, the thread shows the file's name and
  size instead.
- When an upload fails, the front posts the reason. If Slack refuses that
  post, the front posts it on a later try. The front tries the upload again,
  with a wait that grows from 2 seconds up to 64. A retry waits by itself and
  does not hold back other threads. 10 minutes after the first failure, the
  front stops and posts that it gave up, with the reason, when Slack accepts
  that post. The front never tries that version again, not even after a
  restart.
- After `idle_minutes` with no turn and no waiting message, the front ends the
  thread's host Session. The front keeps the host Session in these cases: a
  Box is starting or closing, an agent works or waits on a question, or the
  controller cannot report a Box's state. Ending the host Session leaves every
  Box as it is. The starter's next message opens a new host Session.

## Where the front keeps secrets

- `/moor login` stores no token in the state directory. After the account
  accepts the token, the front sends the controller `enroll` and then
  `secret-set`, each as the person who pasted the token. The controller keeps
  the secret in its vault under the name `claude` or `github`.
- The `/moor login claude` check gets the pasted token on a pipe at file
  descriptor 3. The token is never in the check's arguments, its environment,
  `slack.db`, or the log. The front stores the token only when the check
  passes.
- A coordinator turn never holds a token. Before the turn starts, the front
  sends the controller `proxy-lease` as the starter. The controller grants a
  lease of the front Proxy for the starter's moor user. The turn's
  `HTTPS_PROXY` carries the lease as the proxy password. The Proxy puts the
  starter's `claude` secret on each request to `api.anthropic.com`. The front
  sends `proxy-release` when the turn ends. A lease also ends after 15 minutes
  and when the controller restarts.
- The front needs `proxy_rules: [claude]` in its controller `fronts` entry. See
  [config.md](config.md#fronts). Without it, every turn says that the
  controller gave this front no Proxy.
- The front asks a starter with no `claude` secret to run `/moor login`. If the
  Proxy is down at the start of a turn, or a turn fails and the controller says
  the Proxy is down, the thread says that the Proxy is not running and that
  moor is restarting it.
- A turn runs in `/` with a cleared environment and no built-in tools. It gets
  its own Claude config directory for each Slack user, and
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
| `api_base` | URL | `https://slack.com/api` | The Slack Web API base. It must start with `https://`, `http://127.0.0.1:`, or `http://localhost:`. A loopback base works only when you run moor-slack yourself. A front that the controller runs on Linux has a loopback of its own. |
| `github_api_base` | URL | `https://api.github.com` | The GitHub API base. The front checks a pasted `gh auth token` here with `GET /user`. The same URL rules as `api_base` apply. |
| `long_turn_minutes` | integer, at least 1 | 20 | Minutes that one Box turn can run before the front posts a line about it in the thread. The front posts again after each further `long_turn_minutes` of the same turn. |
| `coordinator.binary` | absolute path | `claude` on `PATH` | The Claude Code binary. |
| `coordinator.agent` | `claude` | `claude` | The coordinator agent. |
| `coordinator.model` | string | `sonnet` | The model for each turn. |
| `coordinator.context_kib` | integer, at least 1 | 32 | The context budget for one turn, in KiB. |
| `coordinator.max_open_boxes` | integer, at least 1 | 1 | The most Boxes that one thread has open at once. |
| `coordinator.idle_minutes` | integer, at least 1 | 30 | Quiet minutes before a thread's host Session ends. |
| `roleplay.users` | list of Slack user ids, at least one | off | For test automation only. The bot can post with its own token and with message metadata `{"event_type": "moor_roleplay", "event_payload": {"user": "U…"}}` for a listed user. The front then counts the post as that user's. A top-level post that names the bot starts a thread, and a thread post is a reply. Anyone who holds the bot token can then act as the listed users. The front logs each such message with its channel, ts, and user. |

## Your own MCP servers

Each person can give their Boxes their own MCP servers with `/moor mcp add`.
The form asks for a name, an `https://` URL, and a key, which is a bearer token
or an API key. Leave the key empty for a server that lets you sign in instead,
such as Linear, Notion or GitHub. The key field shows what you type, to you only. The front sends
the form to the controller as you. The controller keeps the key in your vault,
and a Box sees only a placeholder. The servers in the operator's `mcp_servers`
stay shared defaults for everyone. If one of your servers has the same name,
yours wins. `/moor mcp list` and `/moor mcp remove NAME` answer only you. A
refused form stays open with the reason under the field. For example, the
front refuses a name that is not allowed or a URL that is not `https://`.

When you leave the key empty and the server asks you to sign in, the form turns
into a Sign in button that opens the server's sign-in page, and a field named
Address after sign-in. Approve there. The page may not load, because the
address the server sends you back to may not be reachable from your browser.
That is fine. Copy the address from your browser's address bar, paste it into
the field and choose Finish sign-in. Moor then says that the server is
connected. If the sign-in ran out of time, belongs to someone else or came from
a different server than the one you started with, the reason shows under the
field, and you can run `/moor mcp add` again. A server of yours that you
signed in to never shows its token to a Box.

To keep a server read-only, point it at a read-only URL and give it a key that
can only read. For Linear, use the URL `https://mcp.linear.app/mcp/readonly`
with a personal API key that has only Read permission:

1. In Linear, open Settings, then Security & access, then Personal API keys.
2. Create a key and choose Only select permissions.
3. Tick Read and nothing else, then copy the key.
4. Run `/moor mcp add`. Enter a name such as `linear`, the URL above, and the
   key.

## Your own machines

Each person enrolls their own machines from the machine itself, with
`moor-machine-agent enroll --server <url> --name <name>`, then keeps
`moor-machine-agent run --allow-exec` running there. See
[Enrolling a machine](machines.md#enrolling-a-machine). `/moor machine add`
replies with these steps. moor stores only the machine's public key.
`/moor machine list` and `/moor machine remove NAME` answer only you and cover
only your own machines.

## Your agent profile

`/moor profile set https://github.com/OWNER/NAME[#REF]` records the GitHub repo
you keep your agent profile in. Each new Box you start applies it; see
[Per-user agent profile](config.md#per-user-agent-profile). The front sends the
command to the controller as you, and the controller's refusal, for example for
a URL that is not on github.com, is shown word for word. `/moor profile` and
`/moor profile remove` answer only you.

## Slash commands

| Command | Who | Where | What it does |
| --- | --- | --- | --- |
| `/moor repo` | anyone | any channel the bot is in | Lists the channel's repos and marks the default. |
| `/moor repo add OWNER/NAME` | anyone | any channel the bot is in | Adds a GitHub repo. The front refuses a name that is not `OWNER/NAME`. The controller refuses to start a repo that the person's GitHub account cannot read. |
| `/moor repo remove NAME` | anyone | any channel the bot is in | Removes a repo. |
| `/moor repo default NAME` | anyone | any channel the bot is in | Makes `NAME` the channel's default repo. |
| `/moor login [claude\|github]` | anyone | any channel | Opens a form for the tokens. With no word, the form has both optional fields. With `claude` or `github`, it has only that field. The front checks a Claude token with one test call to Claude on `haiku`. It checks a GitHub token with `GET /user`, which must answer 200. It then stores the token in the controller's vault as you. A 401 or 403 stores nothing. |
| `/moor logout [claude\|github]` | anyone | any channel | Deletes that secret from the vault and tells you whether one was there. The default is `claude`. |
| `/moor mcp add` | anyone | any channel | Opens a private form for an MCP server of yours with the fields Name, URL, and Key. See [Your own MCP servers](#your-own-mcp-servers). |
| `/moor mcp list` | anyone | any channel | Shows only you each server's name, URL, and whether a key is stored. It never shows a key. |
| `/moor mcp remove NAME` | anyone | any channel | Removes that server and its key, and tells only you. |
| `/moor machine add` | anyone | any channel | Replies, only to you, with how to enroll a machine of yours. See [Your own machines](#your-own-machines). |
| `/moor machine list` | anyone | any channel | Shows only you each machine's name, ssh target, and labels. |
| `/moor machine remove NAME` | anyone | any channel | Removes that machine, and tells only you whether there was one. |
| `/moor profile` | anyone | any channel | Shows only you your agent profile repo and ref, or that none is set. |
| `/moor profile set URL[#REF]` | anyone | any channel | Records your agent profile repo, which each new Box applies. Answers only you with the stored value or the controller's refusal. See [Your agent profile](#your-agent-profile). |
| `/moor profile remove` | anyone | any channel | Clears your profile, and tells only you whether one was set. |
