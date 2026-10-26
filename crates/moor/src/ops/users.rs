//! User requests, run as an identity the controller already checked against its socket.

use std::fmt::Write as _;
use std::time::Duration;

use moor_core::request::UserCmd;
use moor_core::user::{Identity, SecretName, UserId};
use moor_core::view::user::{
    EnrollValue, LinkCodeValue, LinkedValue, SecretChangeValue, SecretNamesValue, WhoamiValue,
};
use moor_core::{ErrorKind, Secret};

use super::host_session::refuse_invalid;
use super::to_value;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Store;
use crate::vault::Vault;

mod machine;
pub(crate) mod mcp;

/// How long a link code stays redeemable.
const LINK_CODE_TTL: Duration = Duration::from_secs(600);

/// Run `cmd` as `who`.
pub(crate) fn run(
    cx: &Cx,
    vault: &Vault,
    who: &Identity,
    cmd: &UserCmd,
) -> Result<serde_json::Value, CommandError> {
    let store = cx.store()?;
    match cmd {
        UserCmd::Whoami => whoami(store, who),
        UserCmd::Enroll => {
            let (user, created) = store.enroll(who)?;
            to_value(&EnrollValue { user, created })
        }
        UserCmd::SecretSet { name, value } => {
            let user = enrolled(store, who)?;
            set_secret(store, vault, user, name, value.secret())
        }
        UserCmd::SecretDelete { name } => {
            let changed = store.delete_user_secret(enrolled(store, who)?, name)?;
            to_value(&SecretChangeValue {
                name: name.clone(),
                changed,
            })
        }
        UserCmd::SecretNames => {
            let names = store.user_secret_names(enrolled(store, who)?)?;
            to_value(&SecretNamesValue { names })
        }
        UserCmd::McpAdd {
            name,
            url,
            header,
            key,
        } => {
            let user = enrolled(store, who)?;
            mcp::add(
                cx,
                vault,
                user,
                (name, url, header.as_deref()),
                key.as_ref(),
            )
        }
        UserCmd::McpRemove { name } => mcp::remove(store, enrolled(store, who)?, name),
        UserCmd::McpList => mcp::list(store, enrolled(store, who)?),
        UserCmd::MachineAdd { .. } | UserCmd::MachineRemove { .. } | UserCmd::MachineList => {
            machine::run(store, enrolled(store, who)?, cmd)
        }
        UserCmd::LinkCode => link_code(cx, store, enrolled(store, who)?),
        UserCmd::ProxyLease => super::front_proxy::lease(cx, store, who),
        UserCmd::ProxyRelease { token } => super::front_proxy::release(cx, store, who, token),
        UserCmd::ProxyStatus => super::front_proxy::status(cx, who),
        UserCmd::LinkRedeem { code } => {
            let user = store.redeem_link_code(code, who).map_err(refuse_invalid)?;
            to_value(&LinkedValue {
                user,
                identity: who.clone(),
            })
        }
    }
}

/// `link-code`: mint a one-time code for `user`.
fn link_code(cx: &Cx, store: &Store, user: UserId) -> Result<serde_json::Value, CommandError> {
    let code = mint_code()?;
    let expires = cx.clock().now().saturating_add(LINK_CODE_TTL);
    store.add_link_code(user, &code, expires)?;
    to_value(&LinkCodeValue {
        code,
        expires_in_secs: LINK_CODE_TTL.as_secs(),
    })
}

/// `whoami`: `who`, its user, and every identity linked to that user.
fn whoami(store: &Store, who: &Identity) -> Result<serde_json::Value, CommandError> {
    let user = store.user_of(who)?;
    let identities = match user {
        Some(user) => store.identities(user)?,
        None => Vec::new(),
    };
    to_value(&WhoamiValue {
        identity: who.clone(),
        user,
        identities,
    })
}

/// `secret-set`: seal `value` and store it as `user`'s secret `name`.
fn set_secret(
    store: &Store,
    vault: &Vault,
    user: UserId,
    name: &SecretName,
    value: &Secret,
) -> Result<serde_json::Value, CommandError> {
    if value.expose().is_empty() {
        return Err(CommandError::new(
            ErrorKind::Usage,
            "the secret value is empty",
        ));
    }
    let sealed = vault
        .seal(user, name, value)
        .map_err(|e| CommandError::new(ErrorKind::Failed, e))?;
    store.set_user_secret(user, name, &sealed)?;
    to_value(&SecretChangeValue {
        name: name.clone(),
        changed: true,
    })
}

/// The user `who` is linked to; refused when it has none.
fn enrolled(store: &Store, who: &Identity) -> Result<UserId, CommandError> {
    store.user_of(who)?.ok_or_else(|| {
        CommandError::new(ErrorKind::Refused, format!("{who} has no user"))
            .with_hint("run `moor user enroll`, or redeem a link code from a linked identity")
    })
}

/// A fresh link code: 16 lowercase hex digits from the OS random source.
fn mint_code() -> Result<String, CommandError> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|e| {
        CommandError::new(ErrorKind::Failed, format!("cannot mint a link code: {e}"))
    })?;
    Ok(bytes.iter().fold(String::with_capacity(16), |mut code, b| {
        let _ = write!(code, "{b:02x}");
        code
    }))
}

#[cfg(test)]
mod tests;
