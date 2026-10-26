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
use crate::store::Store;
use crate::vault::Vault;

/// `mcp-add`: check the server, store `key` as its secret when given, then list it.
pub(super) fn add(
    cx: &Cx,
    vault: &Vault,
    user: UserId,
    (name, url, header): (&str, &str, Option<&str>),
    key: Option<&SecretValue>,
    machine: bool,
) -> Result<serde_json::Value, CommandError> {
    let server = UserMcp::parse(name, url, header)
        .map_err(|e| CommandError::new(ErrorKind::Usage, e))?
        .for_machine(machine);
    let rule = format!("mcp-{name}");
    if cx.config().proxy.rules.contains_key(&rule) {
        return Err(CommandError::new(
            ErrorKind::Usage,
            format!("the server name `{name}` is taken by the Proxy rule {rule}"),
        ));
    }
    let store = cx.store()?;
    if let Some(key) = key {
        set_secret(store, vault, user, &server.secret(), key.secret())?;
    }
    store.set_user_mcp(user, &server)?;
    to_value(&McpChangeValue {
        name: server.name.clone(),
        changed: true,
        secret: server.secret(),
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
    to_value(&McpChangeValue {
        name: name.to_owned(),
        changed,
        secret: server.secret(),
    })
}

/// `mcp-list`: names, URLs and whether a key is stored; never a key.
pub(super) fn list(store: &Store, user: UserId) -> Result<serde_json::Value, CommandError> {
    let have = held(store, user)?;
    let servers = store
        .user_mcp(user)?
        .into_iter()
        .map(|s| McpServerView {
            has_key: have.contains(&s.secret()),
            secret: s.secret(),
            machine: s.machine,
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
/// less any whose key the user lacks and any for machine Boxes when `machine` is false. A Box
/// with no user gets the shared ones it can resolve.
///
/// # Errors
/// The store cannot be read.
pub(crate) fn for_box(
    cx: &Cx,
    user: Option<UserId>,
    machine: bool,
) -> Result<BoxMcp, CommandError> {
    let shared = &cx.config().mcp_servers;
    let Some(user) = user else {
        return Ok(box_mcp(shared, &[], &BTreeSet::new(), machine));
    };
    let store = cx.store()?;
    Ok(box_mcp(
        shared,
        &store.user_mcp(user)?,
        &held(store, user)?,
        machine,
    ))
}

#[cfg(test)]
mod tests;
