//! Parsing of Socket Mode envelopes into what the worker consumes.

use moor_core::secret::Secret;
use serde_json::Value;

use super::roleplay::roleplayed;
use super::{Click, Mention, Press, Roleplay, Slash, TokenSubmission};
use crate::login::LOGIN_CALLBACK;
use crate::mcp::MCP_CALLBACK;
use crate::stop::STOP_ACTION;

/// The command inside a `slash_commands` envelope, if it is a well-formed one.
pub(super) fn parse_slash(envelope: &Value) -> Option<Slash> {
    let payload = envelope.get("payload")?;
    let text = |value: Option<&Value>| value?.as_str().map(str::to_owned);
    Some(Slash {
        envelope_id: text(envelope.get("envelope_id"))?,
        channel: text(payload.get("channel_id"))?,
        user: text(payload.get("user_id"))?,
        text: text(payload.get("text")).unwrap_or_default(),
        trigger_id: text(payload.get("trigger_id")),
        response_url: text(payload.get("response_url")),
    })
}

/// The login modal inside an `interactive` `view_submission` envelope, if it is one.
pub(super) fn parse_submission(envelope: &Value) -> Option<TokenSubmission> {
    let payload = envelope.get("payload")?;
    if payload.get("type")?.as_str()? != "view_submission" {
        return None;
    }
    let view = payload.get("view")?;
    if ![LOGIN_CALLBACK, MCP_CALLBACK].contains(&view.get("callback_id")?.as_str()?) {
        return None;
    }
    let tokens = view
        .get("state")?
        .get("values")?
        .as_object()?
        .iter()
        .filter_map(|(block, actions)| {
            let input = actions.get(block)?;
            let value = input.get("value").and_then(Value::as_str);
            let token = Secret::new(value.unwrap_or_default().trim().as_bytes().to_vec());
            Some((block.clone(), token))
        })
        .collect();
    let text = |value: Option<&Value>| value?.as_str().map(str::to_owned);
    Some(TokenSubmission {
        envelope_id: text(envelope.get("envelope_id"))?,
        user: text(payload.get("user").and_then(|u| u.get("id")))?,
        view_id: text(view.get("id"))?,
        account: text(view.get("private_metadata")).unwrap_or_default(),
        tokens,
    })
}

/// What the button of `action` asks for: Stop on a Box, or an answer by option number.
fn parse_press(action: &Value) -> Option<Press> {
    let id = action.get("action_id")?.as_str()?;
    let value = action.get("value")?.as_str()?;
    if id == STOP_ACTION {
        return (!value.is_empty()).then(|| Press::Stop(value.to_owned()));
    }
    if !id.starts_with("answer_") {
        return None;
    }
    match value.parse::<u32>().ok()? {
        0 => None,
        choice => Some(Press::Answer(choice)),
    }
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
    let press = parse_press(action)?;
    Some(Click {
        envelope_id: text(envelope.get("envelope_id"))?,
        channel: text(payload.get("channel").and_then(|c| c.get("id")))
            .or_else(|| text(container.and_then(|c| c.get("channel_id"))))?,
        user: text(payload.get("user").and_then(|u| u.get("id")))?,
        message_ts: text(container.and_then(|c| c.get("message_ts")))
            .or_else(|| text(message.and_then(|m| m.get("ts"))))?,
        thread_ts: text(message.and_then(|m| m.get("thread_ts"))),
        press,
    })
}

/// The mention or thread reply inside an `events_api` envelope, if it is a well-formed one.
/// With `roleplay` set, a post of the bot's own that carries role-play metadata counts as one
/// from the user the metadata names.
pub(super) fn parse_mention(envelope: &Value, roleplay: Option<&Roleplay>) -> Option<Mention> {
    let payload = envelope.get("payload")?;
    let event = payload.get("event")?;
    let field = |value: &Value, key: &str| value.get(key)?.as_str().map(str::to_owned);
    if event.get("bot_id").is_some() {
        return roleplayed(payload, event, roleplay?);
    }
    let reply = match event.get("type")?.as_str()? {
        "app_mention" => false,
        "message" => user_reply(payload, event)?,
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

/// True for a `message` event that is a user's plain thread reply.
fn user_reply(payload: &Value, event: &Value) -> Option<bool> {
    let field = |value: &Value, key: &str| value.get(key)?.as_str().map(str::to_owned);
    // Subtyped events (edits, deletes, joins) are not user replies.
    if event.get("subtype").is_some() {
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
    Some(true)
}
