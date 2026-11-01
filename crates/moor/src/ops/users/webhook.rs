//! A user's webhooks: `webhook-add`, `webhook-list`, `webhook-remove`.

use std::fmt::Write as _;

use moor_core::domain::event::EVENT_KINDS;
use moor_core::user::UserId;
use moor_core::view::user::{WebhookAddValue, WebhookListValue, WebhookRemoveValue};
use moor_core::{ErrorKind, Secret};

use super::to_value;
use crate::error::CommandError;
use crate::store::Store;
use crate::vault::Vault;

/// The event kinds a webhook gets when it names none: a turn ended, or a Box or its agent
/// stopped in a way someone must look at.
const DEFAULT_EVENTS: [&str; 8] = [
    "idled",
    "needs_input",
    "closed",
    "setup_failed",
    "session_failed",
    "launch_timed_out",
    "relaunch_capped",
    "box_failed",
];

/// Most webhooks one user may hold.
const MAX_WEBHOOKS: usize = 10;

/// Most event kinds one webhook may name.
const MAX_EVENTS: usize = 64;

/// The refusal for an id that is not the caller's, or does not exist.
const NO_SUCH: &str = "no such webhook";

/// `webhook-add`: mint a key, store the webhook sealed, and return the key once.
pub(super) fn add(
    store: &Store,
    vault: &Vault,
    user: UserId,
    url: &str,
    events: Option<&[String]>,
) -> Result<serde_json::Value, CommandError> {
    check_url(url).map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    let events = match events {
        Some(named) => checked_events(named)?,
        None => DEFAULT_EVENTS.map(str::to_owned).to_vec(),
    };
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| {
        CommandError::new(ErrorKind::Failed, format!("cannot mint a webhook key: {e}"))
    })?;
    let key = bytes.iter().fold(String::with_capacity(64), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    });
    let secret = Secret::new(key.clone().into_bytes());
    let id = store
        .add_user_webhook(vault, user, (url, &events), &secret, MAX_WEBHOOKS)?
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::Refused,
                format!("at most {MAX_WEBHOOKS} webhooks per user"),
            )
        })?;
    to_value(&WebhookAddValue { id, key })
}

/// `webhook-list`: the caller's webhooks, without keys.
pub(super) fn list(store: &Store, user: UserId) -> Result<serde_json::Value, CommandError> {
    to_value(&WebhookListValue {
        webhooks: store.user_webhooks(user)?,
    })
}

/// `webhook-remove`: delete the caller's webhook `id`.
pub(super) fn remove(
    store: &Store,
    user: UserId,
    id: i64,
) -> Result<serde_json::Value, CommandError> {
    if !store.delete_user_webhook(user, id)? {
        return Err(CommandError::new(ErrorKind::Refused, NO_SUCH));
    }
    to_value(&WebhookRemoveValue { id })
}

/// `named` as a deduplicated list of real event kinds, in the order given.
fn checked_events(named: &[String]) -> Result<Vec<String>, CommandError> {
    let usage = |e: String| CommandError::new(ErrorKind::Usage, e);
    if named.is_empty() {
        return Err(usage("events: name at least one event kind".to_owned()));
    }
    if named.len() > MAX_EVENTS {
        return Err(usage(format!("events: at most {MAX_EVENTS} event kinds")));
    }
    let mut out: Vec<String> = Vec::with_capacity(named.len());
    for kind in named {
        if !EVENT_KINDS.contains(&kind.as_str()) {
            return Err(usage(format!("events: `{kind}` is not an event kind")));
        }
        if !out.contains(kind) {
            out.push(kind.clone());
        }
    }
    Ok(out)
}

/// An `https://` URL with a host, or an `http://` one on the loopback.
fn check_url(url: &str) -> Result<(), String> {
    let parsed: ureq::http::Uri = url
        .parse()
        .map_err(|_| format!("url: `{url}` is not a URL"))?;
    let host = parsed.host().unwrap_or_default();
    match parsed.scheme_str() {
        Some("https") if !host.is_empty() => Ok(()),
        Some("http") if matches!(host, "127.0.0.1" | "localhost" | "[::1]") => Ok(()),
        _ => Err(format!(
            "url: `{url}` must be https://, or http:// on 127.0.0.1, [::1] or localhost"
        )),
    }
}

#[cfg(test)]
mod tests;
