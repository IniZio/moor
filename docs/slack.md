# Slack app

Let teammates start and steer Boxes from Slack. `moor-slack` answers Slack
over Socket Mode, so it needs no public URL. A mention in a channel opens a
thread with a coordinator agent, which starts Boxes for the person who asked.

You need a running controller, Claude Code 2.1.304 or newer on the controller
host, and a Slack workspace where you can create an app.

## Create the Slack app

1. Create a Slack app and turn on Socket Mode.
2. Create an app-level token with the `connections:write` scope.
3. Give the bot token the scopes `app_mentions:read`, `channels:history`,
   `chat:write`, `commands`, `files:read`, `files:write`, `reactions:write`,
   `users:read`, and `users:read.email`.
4. Add a `/moor` slash command.
5. Subscribe to the `app_mention` and `message.channels` bot events. Without
   `message.channels`, the bot never sees a reply in a thread.
6. Turn on Interactivity. Socket Mode needs no request URL.
7. Install the app in the workspace.

The two `users:read` scopes let the front read each person's Slack email. The
same email in Slack and at your sign-in provider makes one moor user.

## Write the front's config

1. Put each token in a file only you can read:

   ```sh
   mkdir -p ~/.config/moor-slack
   install -m 600 /dev/null ~/.config/moor-slack/app-token
   install -m 600 /dev/null ~/.config/moor-slack/bot-token
   ```

   Write the `xapp-` token into `app-token` and the `xoxb-` token into
   `bot-token`.

2. Write `~/.config/moor-slack/config.yaml`:

   ```yaml
   app_token_file: /home/me/.config/moor-slack/app-token
   bot_token_file: /home/me/.config/moor-slack/bot-token
   ```

The config has no channel list. The bot answers in any channel it is invited
to, so channel membership decides where it works.

## Let the controller run the front

Each coordinator turn runs Claude Code with the person's own `claude` secret,
which reaches it through the front Proxy. Only a front the controller runs gets
that Proxy. Add the front to `~/.config/moor/controller.yaml`:

```yaml
fronts:
  slack:
    command: [moor-slack, --config, /home/me/.config/moor-slack/config.yaml]
    state_dir: /home/me/.local/state/moor-slack
    read_only: [/home/me/.config/moor-slack]
    proxy_rules: [claude]
```

Then enable its socket and restart the controller:

```sh
systemctl --user enable --now moor-front@slack.socket
systemctl --user restart moor.service
```

The controller log shows each line the front writes, prefixed
`front slack:`. The front logs the Claude Code version it found. Without a
`claude` binary, or with one older than 2.1.304, the coordinator is off and
each thread says why.

On Kubernetes, set `slack.enabled`, `slack.appToken`, and `slack.botToken`
in the chart instead.

## Try it

1. Invite the bot to a channel with `/invite @moor`.
2. Run `/moor repo add OWNER/NAME` there.
3. Mention the bot with some work.

The bot answers in a thread. A person with no `claude` secret is asked to
link one. [Fronts](fronts.md#moor-slack) lists the front's flags, its config
keys, and every slash command.
