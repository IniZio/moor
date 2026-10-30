//! User requests, run as an identity the controller already checked against its socket.

use moor_core::request::UserCmd;
use moor_core::user::{Identity, SecretName, UserId};
use moor_core::view::user::{EnrollValue, SecretChangeValue, SecretNamesValue, WhoamiValue};
use moor_core::{ErrorKind, Secret};

use super::to_value;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Store;
use crate::vault::Vault;

mod machine;
pub(crate) mod mcp;
mod profile;

/// Run `cmd` as `who`.
#[expect(clippy::too_many_lines, reason = "one arm per user command")]
pub(crate) fn run(
    cx: &Cx,
    vault: &Vault,
    who: &Identity,
    cmd: &UserCmd,
) -> Result<serde_json::Value, CommandError> {
    let store = cx.store()?;
    match cmd {
        UserCmd::Whoami => whoami(store, who),
        UserCmd::Enroll { email } => {
            let (user, created) = store.enroll(who, email.as_ref())?;
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
        UserCmd::McpOAuthFinish { .. } => mcp::oauth::finish(cx, vault, enrolled(store, who)?, cmd),
        UserCmd::McpList => mcp::list(store, enrolled(store, who)?),
        UserCmd::ProfileSet { source } => profile::set(store, enrolled(store, who)?, source),
        UserCmd::ProfileShow => profile::show(store, enrolled(store, who)?),
        UserCmd::ProfileRemove => profile::remove(store, enrolled(store, who)?),
        UserCmd::MachineEnroll {
            email,
            name,
            pubkey,
            labels,
            capacity,
            pool,
        } => machine::enroll(
            store,
            who,
            email.as_ref(),
            (name, pubkey, labels),
            (*capacity, pool.as_deref()),
        ),
        UserCmd::MachineAdd { .. } | UserCmd::MachineRemove { .. } | UserCmd::MachineList => {
            machine::run(store, enrolled(store, who)?, cmd)
        }
        UserCmd::ProxyLease => super::front_proxy::lease(cx, store, who),
        UserCmd::ProxyRelease { token } => super::front_proxy::release(cx, store, who, token),
        UserCmd::ProxyStatus => super::front_proxy::status(cx, who),
    }
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
            .with_hint("run `moor user enroll`")
    })
}

#[cfg(test)]
mod tests;
