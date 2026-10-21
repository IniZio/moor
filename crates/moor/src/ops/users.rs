//! User requests, run as an identity the controller already checked against its socket.

use moor_core::request::UserCmd;
use moor_core::user::Identity;
use moor_core::view::user::WhoamiValue;

use super::to_value;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::vault::Vault;

/// Run `cmd` as `who`.
pub(crate) fn run(
    cx: &Cx,
    _vault: &Vault,
    who: &Identity,
    cmd: &UserCmd,
) -> Result<serde_json::Value, CommandError> {
    let store = cx.store()?;
    match cmd {
        UserCmd::Whoami => {
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
    }
}

#[cfg(test)]
mod tests;
