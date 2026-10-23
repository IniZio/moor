//! `proxy-lease`, `proxy-release` and `proxy-status`: a front leases its Proxy for the user an
//! identity in its own namespace belongs to. The lease token is the only thing the front gets;
//! the Proxy injects the user's secrets in its place.

use std::fmt::Write as _;
use std::time::Duration;

use moor_core::config::proxy::front_rules;
use moor_core::request::SecretValue;
use moor_core::user::{Identity, SecretName};
use moor_core::view::user::{ProxyLeaseValue, ProxyReleaseValue, ProxyStatusValue};
use moor_core::ErrorKind;

use super::to_value;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Store;

/// How long a lease lives unless it is released first: longer than any one turn of a front.
const LEASE_TTL: Duration = Duration::from_mins(15);

/// `proxy-lease` as `who`.
pub(crate) fn lease(
    cx: &Cx,
    store: &Store,
    who: &Identity,
) -> Result<serde_json::Value, CommandError> {
    let front = front_of(cx, who)?;
    let needed = secrets(cx, front)?;
    let Some(user) = store.user_of(who)? else {
        let name = needed.into_iter().next().ok_or_else(no_rules)?;
        return to_value(&ProxyLeaseValue::NoSecret { name });
    };
    let names = store.user_secret_names(user)?;
    if let Some(name) = needed.into_iter().find(|n| !names.contains(n)) {
        return to_value(&ProxyLeaseValue::NoSecret { name });
    }
    if !cx.proxy_host().front_running(front) {
        return to_value(&ProxyLeaseValue::ProxyDown);
    }
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| CommandError::new(ErrorKind::Failed, format!("cannot mint a lease: {e}")))?;
    let token = bytes.iter().fold(String::with_capacity(64), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    });
    let expires = cx.clock().now().saturating_add(LEASE_TTL);
    store.add_front_lease(token.as_bytes(), front, user, expires)?;
    to_value(&ProxyLeaseValue::Granted {
        token: SecretValue::new(token.into_bytes()),
        expires_in_secs: LEASE_TTL.as_secs(),
    })
}

/// `proxy-release` of `token` as `who`.
pub(crate) fn release(
    cx: &Cx,
    store: &Store,
    who: &Identity,
    token: &SecretValue,
) -> Result<serde_json::Value, CommandError> {
    let front = front_of(cx, who)?;
    let released = match store.user_of(who)? {
        Some(user) => store.delete_front_lease(token.secret().expose(), front, user)?,
        None => false,
    };
    to_value(&ProxyReleaseValue { released })
}

/// `proxy-status` as `who`.
pub(crate) fn status(cx: &Cx, who: &Identity) -> Result<serde_json::Value, CommandError> {
    let front = front_of(cx, who)?;
    to_value(&ProxyStatusValue {
        running: cx.proxy_host().front_running(front),
    })
}

/// The front `who` speaks for: its namespace, which must have a Proxy.
fn front_of<'a>(cx: &Cx, who: &'a Identity) -> Result<&'a str, CommandError> {
    let front = who.namespace();
    if cx.env().front_proxy_rules(front).is_empty() {
        return Err(no_rules().with_hint(format!(
            "only a front whose controller config lists proxy_rules has a Proxy, and {who} is not one"
        )));
    }
    Ok(front)
}

/// The secrets the rules of `front`'s Proxy inject, in rule order.
fn secrets(cx: &Cx, front: &str) -> Result<Vec<SecretName>, CommandError> {
    let known = front_rules();
    let mut names: Vec<SecretName> = Vec::new();
    for rule in cx.env().front_proxy_rules(front) {
        let found = known
            .iter()
            .find(|r| r.name == *rule)
            .ok_or_else(no_rules)?;
        let name = SecretName::parse(&found.credential)
            .map_err(|e| CommandError::new(ErrorKind::Internal, e))?;
        if !names.contains(&name) {
            names.push(name);
        }
    }
    Ok(names)
}

/// The refusal for a caller whose front has no Proxy.
fn no_rules() -> CommandError {
    CommandError::new(ErrorKind::Refused, "this front has no Proxy")
}
