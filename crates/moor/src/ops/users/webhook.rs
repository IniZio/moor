//! A user's webhooks: `webhook-add`, `webhook-list`, `webhook-remove`, and the
//! `webhook-setup-show` and `webhook-setup-finish` the gate's setup page sends, so its key and
//! header value pass only between the person and moor.

use std::fmt::Write as _;

use moor_core::domain::event::EVENT_KINDS;
use moor_core::request::SecretValue;
use moor_core::user::UserId;
use moor_core::view::user::{
    WebhookAddValue, WebhookListValue, WebhookRemoveValue, WebhookSetupFinishValue,
    WebhookSetupValue, WebhookState,
};
use moor_core::{ErrorKind, Secret};

use super::to_value;
use crate::error::CommandError;
use crate::store::{webhook_key_name, Store};
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

/// Longest header name a webhook may name.
const MAX_HEADER_NAME: usize = 64;

/// Longest header value, in bytes.
const MAX_HEADER_VALUE: usize = 4096;

/// The refusal for an id that is not the caller's, or does not exist.
const NO_SUCH: &str = "no such webhook";

/// `webhook-add`: mint a key and store the webhook sealed, waiting for its person to see the
/// key on the setup page; the reply never holds it.
pub(super) fn add(
    store: &Store,
    vault: &Vault,
    user: UserId,
    (url, events, header): (&str, Option<&[String]>, Option<&str>),
) -> Result<serde_json::Value, CommandError> {
    check_url(url).map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    if let Some(name) = header {
        check_header(name).map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    }
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
    let secret = Secret::new(key.into_bytes());
    let id = store
        .add_user_webhook(vault, user, (url, &events, header), &secret, MAX_WEBHOOKS)?
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::Refused,
                format!("at most {MAX_WEBHOOKS} webhooks per user"),
            )
        })?;
    to_value(&WebhookAddValue {
        id,
        state: WebhookState::WaitingForSecret,
    })
}

/// `webhook-setup-show`: the caller's webhook `id`, with its key only while waiting for setup.
pub(super) fn setup_show(
    store: &Store,
    vault: &Vault,
    user: UserId,
    id: i64,
) -> Result<serde_json::Value, CommandError> {
    let row = store
        .user_webhook_setup(user, id)?
        .ok_or_else(|| CommandError::new(ErrorKind::Refused, NO_SUCH))?;
    let key = match row.state {
        WebhookState::WaitingForSecret => {
            let plain = vault
                .open(user, &webhook_key_name(id)?, &row.sealed_key)
                .map_err(|e| CommandError::new(ErrorKind::Internal, e.to_string()))?;
            Some(SecretValue::new(plain.expose().to_vec()))
        }
        WebhookState::Active => None,
    };
    to_value(&WebhookSetupValue {
        id,
        url: row.url,
        header: row.header,
        state: row.state,
        key,
    })
}

/// `webhook-setup-finish`: seal the header value the webhook needs, if any, and make it active.
pub(super) fn setup_finish(
    store: &Store,
    vault: &Vault,
    user: UserId,
    id: i64,
    header_value: Option<&SecretValue>,
) -> Result<serde_json::Value, CommandError> {
    let refused = |e: String| CommandError::new(ErrorKind::Refused, e);
    let row = store
        .user_webhook_setup(user, id)?
        .ok_or_else(|| refused(NO_SUCH.to_owned()))?;
    let already =
        || refused("already set up; remove and add it again to change its secret".to_owned());
    if row.state == WebhookState::Active {
        return Err(already());
    }
    let value = match (&row.header, header_value) {
        (Some(name), None) => {
            return Err(refused(format!(
                "webhook {id} needs a value for its `{name}` header"
            )));
        }
        (None, Some(_)) => {
            return Err(refused(format!(
                "webhook {id} has no header to give a value"
            )));
        }
        (Some(_), Some(value)) => {
            check_header_value(value.secret().expose())
                .map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
            Some(value.secret())
        }
        (None, None) => None,
    };
    if !store.activate_user_webhook(vault, user, id, value)? {
        return Err(already());
    }
    to_value(&WebhookSetupFinishValue {
        id,
        state: WebhookState::Active,
    })
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

/// `Authorization`, or an `X-` name moor does not set itself, in HTTP token characters.
fn check_header(name: &str) -> Result<(), String> {
    let lower = name.to_ascii_lowercase();
    let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    let shape = lower == "authorization" || (lower.starts_with("x-") && lower.len() > 2);
    if shape
        && name.len() <= MAX_HEADER_NAME
        && name.chars().all(token)
        && !lower.starts_with("x-moor-")
    {
        return Ok(());
    }
    Err(format!(
        "header: `{name}` must be `Authorization` or an `X-` name of at most {MAX_HEADER_NAME} \
         token characters, and not one moor sets (`X-Moor-*`)"
    ))
}

/// A header value: 1 to 4096 bytes, without a line break or other control character.
fn check_header_value(value: &[u8]) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_HEADER_VALUE {
        return Err(format!(
            "header_value: must be 1 to {MAX_HEADER_VALUE} bytes"
        ));
    }
    if value.iter().any(|&b| b != b'\t' && (b.is_ascii_control())) {
        return Err("header_value: must not hold a line break or control character".to_owned());
    }
    Ok(())
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
