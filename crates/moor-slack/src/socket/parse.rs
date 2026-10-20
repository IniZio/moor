//! Parsing of Socket Mode envelopes into what the worker consumes.

use serde_json::Value;

use super::{Click, Mention, Slash};

/// The command inside a `slash_commands` envelope, if it is a well-formed one.
pub(super) fn parse_slash(envelope: &Value) -> Option<Slash> {
    let payload = envelope.get("payload")?;
    let text = |value: Option<&Value>| value?.as_str().map(str::to_owned);
    Some(Slash {
        envelope_id: text(envelope.get("envelope_id"))?,
        channel: text(payload.get("channel_id"))?,
        user: text(payload.get("user_id"))?,
        text: text(payload.get("text")).unwrap_or_default(),
    })
}

/// The button press inside an `interactive` `block_actions` envelope, if it is a well-formed one.
pub(super) fn parse_click(envelope: &Value) -> Option<Click> {
    let payload = envelope.get("payload")?;
    if payload.get("type")?.as_str()? != "block_actions" {
        return None;
    }
    let text = |value: Option<&Value>| value?.as_str().map(str::to_owned);
    let container = payload.get("container");
    let message = payload.get("message");
    let action = payload.get("actions")?.get(0)?;
    if !action.get("action_id")?.as_str()?.starts_with("answer_") {
        return None;
    }
    let choice = action.get("value")?.as_str()?.parse::<u32>().ok()?;
    if choice == 0 {
        return None;
    }
    Some(Click {
        envelope_id: text(envelope.get("envelope_id"))?,
        channel: text(payload.get("channel").and_then(|c| c.get("id")))
            .or_else(|| text(container.and_then(|c| c.get("channel_id"))))?,
        user: text(payload.get("user").and_then(|u| u.get("id")))?,
        message_ts: text(container.and_then(|c| c.get("message_ts")))
            .or_else(|| text(message.and_then(|m| m.get("ts"))))?,
        thread_ts: text(message.and_then(|m| m.get("thread_ts"))),
        choice,
    })
}

/// The mention or thread reply inside an `events_api` envelope, if it is a well-formed one.
pub(super) fn parse_mention(envelope: &Value) -> Option<Mention> {
    let payload = envelope.get("payload")?;
    let event = payload.get("event")?;
    let field = |value: &Value, key: &str| value.get(key)?.as_str().map(str::to_owned);
    let reply = match event.get("type")?.as_str()? {
        "app_mention" => false,
        "message" => {
            // Subtyped events (edits, deletes, joins) and bot posts are not user replies.
            if event.get("subtype").is_some() || event.get("bot_id").is_some() {
                return None;
            }
            let thread_ts = field(event, "thread_ts")?;
            if thread_ts == field(event, "ts")? {
                return None;
            }
            // Slack also delivers a message naming the bot as `app_mention`, which handles it.
            let bot = payload
                .get("authorizations")
                .and_then(|a| a.get(0))
                .and_then(|a| field(a, "user_id"));
            if let Some(bot) = bot {
                if field(event, "text")?.contains(&format!("<@{bot}")) {
                    return None;
                }
            }
            true
        }
        _ => return None,
    };
    Some(Mention {
        event_id: field(payload, "event_id")?,
        channel: field(event, "channel")?,
        user: field(event, "user")?,
        text: field(event, "text")?,
        ts: field(event, "ts")?,
        thread_ts: field(event, "thread_ts"),
        reply,
    })
}
