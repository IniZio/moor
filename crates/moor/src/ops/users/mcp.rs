//! A user's own MCP servers: `mcp-add`, `mcp-remove`, `mcp-list`, and the servers a Box gets.

use std::collections::BTreeSet;

use moor_core::config::{box_mcp, BoxMcp, UserMcp};
use moor_core::request::SecretValue;
use moor_core::user::{SecretName, UserId};
use moor_core::view::user::{McpChangeValue, McpListValue, McpServerView};
use moor_core::ErrorKind;

use super::{set_secret, to_value};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{SignIn, Store};
use crate::vault::Vault;

pub(crate) mod oauth;

/// `mcp-add`: check the server and store `key` as its secret, or with no key start an OAuth
/// sign-in when the server asks for one, then list it.
pub(super) fn add(
    cx: &Cx,
    vault: &Vault,
    user: UserId,
    (name, url, header): (&str, &str, Option<&str>),
    key: Option<&SecretValue>,
) -> Result<serde_json::Value, CommandError> {
    let server =
        UserMcp::parse(name, url, header).map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    let rule = format!("mcp-{name}");
    if cx.config().proxy.rules.contains_key(&rule) {
        return Err(CommandError::new(
            ErrorKind::Usage,
            format!("the server name `{name}` is taken by the Proxy rule {rule}"),
        ));
    }
    let store = cx.store()?;
    let sign_in = match key {
        Some(key) => {
            set_secret(store, vault, user, &server.secret(), key.secret())?;
            None
        }
        None => oauth::start(cx, vault, user, &server)?,
    };
    store.set_user_mcp(user, &server)?;
    to_value(&McpChangeValue {
        name: server.name.clone(),
        changed: true,
        secret: server.secret(),
        sign_in,
    })
}

/// `mcp-remove`: drop the server and its key.
pub(super) fn remove(
    store: &Store,
    user: UserId,
    name: &str,
) -> Result<serde_json::Value, CommandError> {
    let server = UserMcp::parse(name, "https://placeholder.invalid", None)
        .map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    let changed = store.delete_user_mcp(user, name)?;
    store.delete_user_secret(user, &server.secret())?;
    store.delete_user_mcp_oauth(user, name)?;
    to_value(&McpChangeValue {
        name: name.to_owned(),
        changed,
        secret: server.secret(),
        sign_in: None,
    })
}

/// `mcp-list`: names, URLs, whether a key is stored and when a sign-in expires; never a key.
pub(super) fn list(store: &Store, user: UserId) -> Result<serde_json::Value, CommandError> {
    let have = held(store, user)?;
    let signed = store.user_mcp_oauth_state(user)?;
    let servers = store
        .user_mcp(user)?
        .into_iter()
        .map(|s| McpServerView {
            signed_in: match signed.get(&s.name) {
                Some(SignIn::Until(at)) => Some(*at),
                _ => None,
            },
            sign_in_again: signed.get(&s.name) == Some(&SignIn::Ended),
            has_key: have.contains(&s.secret()),
            secret: s.secret(),
            name: s.name,
            url: s.url,
            header: s.header,
        })
        .collect();
    to_value(&McpListValue { servers })
}

/// The names of `user`'s vault secrets.
fn held(store: &Store, user: UserId) -> Result<BTreeSet<SecretName>, CommandError> {
    Ok(store.user_secret_names(user)?.into_iter().collect())
}

/// The MCP servers a Box started by `user` gets: the shared ones merged with the user's own,
/// less any whose key the user lacks. A Box with no user gets the shared ones it can resolve.
pub(crate) fn for_box(cx: &Cx, user: Option<UserId>) -> Result<BoxMcp, CommandError> {
    let shared = &cx.config().shared_mcp();
    let Some(user) = user else {
        return Ok(box_mcp(shared, (&[], &BTreeSet::new()), &BTreeSet::new()));
    };
    let store = cx.store()?;
    let ended = store
        .user_mcp_oauth_state(user)?
        .into_iter()
        .filter_map(|(name, s)| (s == SignIn::Ended).then_some(name))
        .collect();
    Ok(box_mcp(
        shared,
        (&store.user_mcp(user)?, &ended),
        &held(store, user)?,
    ))
}

#[cfg(test)]
mod tests;
