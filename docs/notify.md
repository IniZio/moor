# Get Box events

Find out when a Box needs you without asking over and over. Pick the way that
fits where you work. [Events](events.md) lists the event kinds and the exact
shapes each way delivers.

## In Claude Code

The moor plugin for Claude Code watches each Box it starts and each Box you
ask it to watch. It prints a line in your session when one idles, needs input,
or fails.

The plugin runs the `moor` binary as a local client, so install it and sign in
once:

```sh
curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | sh -s moor
moor login --server https://HOST
export MOOR_SERVER=https://HOST
```

Set `MOOR_SERVER` before you start Claude Code, or in its settings `env`. Then,
inside Claude Code:

```text
/plugin marketplace add IniZio/moor
/plugin install moor@moor
```

The plugin brings its own `moor` MCP server. If you added one with
`claude mcp add`, remove it with `claude mcp remove moor`, so the tools are
not listed twice. The sign-in renews itself.

## At a webhook

A webhook gets a signed `POST` for each event. Use one for a receiver that
holds no open connection, such as a CI job or a chat bot.

1. Ask Claude Code to add a webhook with your receiver's URL. It calls
   `add_webhook` and returns a setup link. The URL must be `https://`.
2. Open the link and sign in. Copy the signing key the page shows into your
   receiver.
3. If your receiver wants its own token in a header, such as
   `Authorization`, enter that value on the page.
4. Click **Finish setup**. Deliveries start with the next event.

The page shows the key only until setup finishes. To change it, remove the
webhook and add it again.

Your receiver checks each request's `X-Moor-Signature`, which is an
HMAC-SHA256 of `<X-Moor-Timestamp>.<body>`. Refuse a timestamp more than a few
minutes old. A delivery can arrive twice, so skip an `X-Moor-Delivery` you have
seen:

```sh
printf '%s.%s' "$timestamp" "$body" | openssl dgst -sha256 -hmac "$key"
```

`list_webhooks` shows each webhook's state and its last delivery failure.
Webhooks need the operator to run the webhook front, as
[Fronts](fronts.md#moor-notify) shows.

## As a line on stdout

`moor listen` prints one line per event that needs a person. Run it as a
background monitor, so each line wakes whatever reads it:

```sh
moor listen --server https://HOST --box fix-typo
```

It uses the sign-in that `moor login` saved. With `--box`, it exits once every
named Box has closed. Without `--box`, it watches every Box you own.

For a token from somewhere else, use the `moor-listen` binary. It runs the
`--token-cmd` command before every connect and reads the token from its stdout:

```sh
moor-listen --url https://HOST/mcp --token-cmd 'cat ~/.config/moor/token'
```

## By polling

A bot can read events from an offset it keeps. Call `list_events` with `after`
set to the last offset you handled. Pass the reply's `next` as `after` next
time. Code that is not an MCP client can call the same thing over HTTP:

```sh
curl -H "Authorization: Bearer $ACCESS_TOKEN" "https://HOST/events?after=0&wait_seconds=30"
```

Each webhook body carries the event's `offset` too, so a receiver that was
down can catch up with `list_events` before it trusts new deliveries.
