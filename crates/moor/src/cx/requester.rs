//! The user a request acts for, and the vault key its Box's Proxy reads that user's secrets with.

use std::path::{Path, PathBuf};

use moor_core::user::{Identity, UserId};
use moor_core::ErrorKind;

use super::Cx;
use crate::error::CommandError;

impl Cx {
    /// This context knowing the controller's vault key file.
    pub(crate) fn with_vault_key(mut self, path: PathBuf) -> Self {
        self.vault_key = Some(path);
        self
    }

    /// This context acting as `identity`, which the socket already checked.
    pub(crate) fn with_requester(mut self, identity: Identity) -> Self {
        self.requester = Some(identity);
        self
    }

    /// The controller's vault key file.
    ///
    /// # Errors
    /// A usage error outside a controller, which alone holds the key.
    pub(crate) fn vault_key(&self) -> Result<&Path, CommandError> {
        self.vault_key
            .as_deref()
            .ok_or_else(|| controller_only("a Box's Proxy reads its user's secrets"))
    }

    /// The identity this request acts as, and its user when it has one.
    ///
    /// # Errors
    /// A usage error outside a controller socket, or the store's failure.
    pub(crate) fn requester(&self) -> Result<(&Identity, Option<UserId>), CommandError> {
        let who = self
            .requester
            .as_ref()
            .ok_or_else(|| controller_only("a Box acts as the user of its caller"))?;
        Ok((who, self.store()?.user_of(who)?))
    }
}

/// The refusal for a request only a controller socket can serve, and why.
fn controller_only(why: &str) -> CommandError {
    CommandError::new(
        ErrorKind::Usage,
        format!("{why}, so only the controller socket serves this request"),
    )
}
