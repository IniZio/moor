//! The user a request acts for, and the vault key its Box's Proxy reads that user's secrets with.

use std::cell::OnceCell;
use std::path::{Path, PathBuf};

use moor_core::request::Grant;
use moor_core::user::{Identity, UserId, Viewer};
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

    /// This context seeing only the Boxes `viewer` may see.
    pub(crate) const fn with_viewer(mut self, viewer: Viewer) -> Self {
        self.viewer = viewer;
        self
    }

    /// This context with each name meaning only the Box the controller checked it against.
    pub(crate) fn with_grant(self, grant: Grant) -> Self {
        let mut cx = self.with_viewer(grant.viewer);
        if let Some(store) = cx.store.take() {
            cx.store = OnceCell::from(store.pinned(grant.pins.clone()));
        }
        cx.pins = grant.pins;
        cx.host_dir = grant.host_dir;
        cx
    }

    /// The one host directory this request reads and writes through; `None` for anywhere.
    pub(crate) fn host_dir(&self) -> Option<&Path> {
        self.host_dir.as_deref()
    }

    /// Whose Boxes this request may see.
    pub(crate) const fn viewer(&self) -> Viewer {
        self.viewer
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
