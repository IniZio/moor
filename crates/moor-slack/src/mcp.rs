//! `/moor mcp add|list|remove`: a person's own MCP servers. `add` opens a modal for the name,
//! URL and key; the key goes into the controller's vault as that Slack user, with the server in
//! their list. These are setup commands; nothing here reads words in a thread.

use moor_core::request::{AsReq, McpAddReq, McpRemoveReq, Request, SecretValue};
use serde_json::{json, Value};

use crate::credentials::SlackUser;
use crate::db::Db;
use crate::login::vault::{send, Call};
use crate::socket::{AfterAck, Kept, Slash, TokenSubmission};
use crate::worker::Job;

/// `callback_id` of the add modal.
pub(crate) const MCP_CALLBACK: &str = "moor_mcp";

/// The add modal's `private_metadata`, which tells a submission from a login one.
pub(crate) const MCP_FORM: &str = "mcp";

/// Shown under the key input.
const KEY_HINT: &str = "Bearer token or API key. Visible while you type, only to you.";

/// Shown above the inputs.
const HELP: &str = "Example: Linear read-only. URL https://mcp.linear.app/mcp/readonly with a \
    Linear personal API key that has only Read permission.";

/// The reply to a `/moor mcp` that is not one of the three.
const USAGE: &str = "Usage: `/moor mcp add` opens a form for a server of yours, \
    `/moor mcp list` shows your servers and `/moor mcp remove <name>` drops one.";

/// The ack of a user whose Slack id the front cannot act for.
pub(crate) const BAD_ID: &str =
    "Your Slack user id is not one moor can act for, so nothing was changed.";

/// The modal `/moor mcp add` opens.
pub(crate) fn form() -> Value {
    let input = |block: &str, label: &str, hint: Option<&str>| {
        let mut input = json!({
            "type": "input",
            "block_id": block,
            "optional": block == "key",
            "label": {"type": "plain_text", "text": label},
            "element": {"type": "plain_text_input", "action_id": block},
        });
        if let (Some(hint), Some(map)) = (hint, input.as_object_mut()) {
            map.insert("hint".into(), json!({"type": "plain_text", "text": hint}));
        }
        input
    };
    json!({
        "type": "modal",
        "callback_id": MCP_CALLBACK,
        "private_metadata": MCP_FORM,
        "title": {"type": "plain_text", "text": "moor mcp"},
        "submit": {"type": "plain_text", "text": "Add"},
        "close": {"type": "plain_text", "text": "Cancel"},
        "blocks": [
            {"type": "context", "elements": [{"type": "plain_text", "text": HELP}]},
            input("name", "Name", None),
            input("url", "URL", None),
            input("key", "Key", Some(KEY_HINT)),
        ],
    })
}

/// A modal that shows `text` as plain text.
fn notice(text: &str) -> Value {
    json!({
        "type": "modal",
        "callback_id": MCP_CALLBACK,
        "title": {"type": "plain_text", "text": "moor mcp"},
        "close": {"type": "plain_text", "text": "Close"},
        "blocks": [{"type": "section", "text": {"type": "plain_text", "text": text}}],
    })
}

/// An ack that shows `text` to the user who ran the command.
pub(crate) fn text_ack(text: &str) -> Kept {
    Kept {
        ack: Some(json!({ "text": text })),
        then: None,
    }
}

/// True for a name made of the characters a server name takes.
pub(crate) fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}

/// A `/moor mcp list` or `/moor mcp remove` for the worker; no `name` means list.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct McpJob {
    /// The Slack user who ran the command.
    pub(crate) user: String,
    /// The server to remove, or none to list.
    pub(crate) name: Option<String>,
    /// Where Slack takes the answer to the command.
    pub(crate) response_url: Option<String>,
}

/// `/moor mcp ...`: `add` opens the modal after an empty ack, `list` and `remove` queue a job
/// whose answer goes to the user alone through the command's `response_url`.
pub(crate) fn slash(slash: &Slash, db: &Db, now: i64) -> rusqlite::Result<(Kept, Option<Job>)> {
    let first = db.atomically(|| db.event_once(&slash.envelope_id, now))?;
    if SlackUser::parse(&slash.user).is_none() {
        return Ok((text_ack(BAD_ID), None));
    }
    let mut words = slash.text.split_whitespace().skip(1);
    let user = slash.user.clone();
    let response_url = slash.response_url.clone();
    let (kept, job) = match (words.next(), words.next(), words.next()) {
        (Some("add"), None, _) => {
            let then = slash
                .trigger_id
                .clone()
                .filter(|_| first)
                .map(|trigger_id| AfterAck::Mcp { trigger_id });
            (Kept { ack: None, then }, None)
        }
        (Some("list"), None, _) => (
            text_ack("Looking up your MCP servers…"),
            Some(Job::Mcp(McpJob {
                user,
                name: None,
                response_url,
            })),
        ),
        (Some("remove"), Some(name), None) if name_ok(name) => (
            text_ack(&format!("Removing `{name}`…")),
            Some(Job::Mcp(McpJob {
                user,
                name: Some(name.to_owned()),
                response_url,
            })),
        ),
        _ => (text_ack(USAGE), None),
    };
    Ok((kept, job.filter(|_| first)))
}

/// What a submission's field named `block` holds, trimmed.
pub(crate) fn field(sub: &TokenSubmission, block: &str) -> String {
    let value = sub.tokens.iter().find(|(b, _)| b == block);
    value.map_or_else(String::new, |(_, v)| {
        String::from_utf8_lossy(v.expose()).into_owned()
    })
}

/// An ack that keeps the modal open with `message` on `block`.
fn on_field(block: &str, message: &str) -> Value {
    json!({"response_action": "errors", "errors": {block: message}})
}

/// The field a controller refusal belongs on: the URL's when it names the URL.
fn refused_field(message: &str) -> &'static str {
    if message.contains("URL") || message.contains("https") {
        "url"
    } else {
        "name"
    }
}

/// Handles a submitted add modal, as the Slack user who submitted it: enrolls them when new,
/// then sends `mcp-add`. The ack is the modal's answer, so a refusal stays on its field.
/// Nothing here logs or returns the key.
pub(crate) fn submission(sub: &TokenSubmission, call: &mut Call<'_>) -> Value {
    let update = |text: &str| json!({"response_action": "update", "view": notice(text)});
    if SlackUser::parse(&sub.user).is_none() {
        return update(&format!("{BAD_ID} Nothing was stored."));
    }
    let (name, url, key) = (field(sub, "name"), field(sub, "url"), field(sub, "key"));
    for (block, value) in [("name", &name), ("url", &url)] {
        if value.is_empty() {
            return on_field(block, "This is required.");
        }
    }
    let add = McpAddReq {
        identity: None,
        header: None,
        key: (!key.is_empty()).then(|| SecretValue::new(key.into_bytes())),
        name: name.clone(),
        url,
    };
    let enroll = Request::Enroll(AsReq { identity: None });
    if let Err(why) = send(call, &sub.user, enroll) {
        return update(&format!("Could not set you up: {why}. Nothing was stored."));
    }
    match send(call, &sub.user, Request::McpAdd(add)) {
        Ok(_) => update(&format!("Added `{name}`. Its key is in your vault.")),
        Err(why) => on_field(refused_field(&why), &why),
    }
}

/// The body of `/moor mcp list` for `user`: each server's name, URL and whether a key is
/// stored, or why the controller could not say.
pub(crate) fn listing(call: &mut Call<'_>, user: &str) -> String {
    let reply = match send(call, user, Request::McpList(AsReq { identity: None })) {
        Ok(reply) => reply,
        Err(why) => {
            return format!(
                "Could not list your MCP servers: {}",
                crate::escape::escape(&why)
            )
        }
    };
    let servers = reply.value.as_ref().and_then(|v| v.get("servers"));
    let servers = servers
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if servers.is_empty() {
        return "You have no MCP servers. Add one with `/moor mcp add`.".to_owned();
    }
    let text = |s: &Value, k: &str| {
        crate::escape::code(s.get(k).and_then(Value::as_str).unwrap_or_default())
    };
    let lines: Vec<String> = servers
        .iter()
        .map(|s| {
            let key = if s.get("has_key").and_then(Value::as_bool) == Some(true) {
                "key stored"
            } else {
                "no key"
            };
            format!("{} {} ({key})", text(s, "name"), text(s, "url"))
        })
        .collect();
    format!("Your MCP servers:\n{}", lines.join("\n"))
}

/// The answer to `/moor mcp remove <name>` for `user`.
pub(crate) fn removal(call: &mut Call<'_>, user: &str, name: &str) -> String {
    let remove = McpRemoveReq {
        identity: None,
        name: name.to_owned(),
    };
    match send(call, user, Request::McpRemove(remove)) {
        Ok(reply) => {
            let changed = reply.value.as_ref().and_then(|v| v.get("changed"));
            if changed.and_then(Value::as_bool) == Some(true) {
                format!("Removed `{name}` and its key.")
            } else {
                format!("You have no MCP server named `{name}`.")
            }
        }
        Err(why) => format!("Could not remove `{name}`: {}", crate::escape::escape(&why)),
    }
}
