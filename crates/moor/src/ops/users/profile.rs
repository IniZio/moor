//! A user's agent profile repo: `profile-set`, `profile-show`, `profile-remove`.

use moor_core::name::ProfileSource;
use moor_core::user::UserId;
use moor_core::view::user::ProfileValue;

use super::to_value;
use crate::error::CommandError;
use crate::store::Store;

/// `profile-set`: record `source`, replacing any earlier one.
pub(super) fn set(
    store: &Store,
    user: UserId,
    source: &ProfileSource,
) -> Result<serde_json::Value, CommandError> {
    store.set_user_profile(user, source)?;
    to_value(&ProfileValue {
        source: Some(source.clone()),
    })
}

/// `profile-show`: the recorded repo, or none.
pub(super) fn show(store: &Store, user: UserId) -> Result<serde_json::Value, CommandError> {
    to_value(&ProfileValue {
        source: store.user_profile(user)?,
    })
}

/// `profile-remove`: clear the recorded repo.
pub(super) fn remove(store: &Store, user: UserId) -> Result<serde_json::Value, CommandError> {
    store.delete_user_profile(user)?;
    to_value(&ProfileValue { source: None })
}

#[cfg(test)]
mod tests;
