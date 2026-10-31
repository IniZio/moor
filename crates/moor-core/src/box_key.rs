//! Every internal identifier of a Box, derived from its [`BoxId`].
//!
//! The Box name is for display and lookup only: slugs and directories
//! come from the id, so a name with `/` or of any legal length is safe.

use crate::name::{BoxId, BoxName};

/// The directory under the Box login home that holds every Box workdir.
pub const WORK_ROOT: &str = "moor";
/// Directory under a machine user's home that holds one state dir per Box.
pub const STATE_ROOT: &str = "moor-state";

/// The start of every directory moor makes directly under the Box home.
pub const HOME_PREFIX: &str = ".moor-";

/// Derives a Box's internal identifiers from its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxKey(BoxId);

impl BoxKey {
    /// The key of the Box `id`.
    #[must_use]
    pub const fn new(id: BoxId) -> Self {
        Self(id)
    }

    /// The Box id.
    #[must_use]
    pub const fn id(self) -> BoxId {
        self.0
    }

    /// The key of the Box a directory under `moor` or `moor-state` is named for: only a plain
    /// positive decimal id, so `07`, `+7` and `7.bak` name none.
    #[must_use]
    pub fn from_dir_name(name: &str) -> Option<Self> {
        let id: i64 = name.parse().ok().filter(|id| *id > 0)?;
        (id.to_string() == name).then(|| Self::new(BoxId::new(id)))
    }

    /// The crabbox slug, `moor-{id}-{nonce}`.
    #[must_use]
    pub fn slug(self, nonce: &str) -> String {
        format!("moor-{}-{nonce}", self.0.get())
    }

    /// The session ref recorded when the Engine reports none, `moor-{id}`.
    #[must_use]
    pub fn session_ref(self) -> String {
        format!("moor-{}", self.0.get())
    }

    /// The lease slug of a Box on a user's pool host, `moor-{id}`; the host's agent reads the
    /// id back from it.
    #[must_use]
    pub fn pool_slug(self) -> String {
        format!("moor-{}", self.0.get())
    }

    /// The credential directory under the Box home, `.moor-cred-{id}-{nonce}`.
    #[must_use]
    pub fn cred_dir(self, nonce: &str) -> String {
        format!("{HOME_PREFIX}cred-{}-{nonce}", self.0.get())
    }

    /// The Box's copy on a user machine, under the machine user's home, `moor/{id}`.
    #[must_use]
    pub fn machine_dir(self) -> String {
        format!("{WORK_ROOT}/{}", self.0.get())
    }

    /// The runner's per-Box state on a user machine, under the machine user's home,
    /// `moor-state/{id}`; holds the Box's temp files and whatever the machine's hooks keep.
    #[must_use]
    pub fn machine_state_dir(self) -> String {
        format!("{STATE_ROOT}/{}", self.0.get())
    }

    /// The scratch directory under the Box home, `.moor-tmp-{id}-{nonce}`.
    #[must_use]
    pub fn tmp_dir(self, nonce: &str) -> String {
        format!("{HOME_PREFIX}tmp-{}-{nonce}", self.0.get())
    }
}

/// The git branch of a new Box named `name`: `moor/{repo}--{label}`.
///
/// The `/` of the name becomes `--`, so the branch holds exactly one `/`.
/// Segments of a `<repo>/<label>` name cannot contain `--`, so the encoding is
/// reversible and two such names never share a branch. A bare name containing
/// `--` can share a branch with a `<repo>/<label>` name; that gap closes when
/// bare names go with the next schema.
#[must_use]
pub fn branch(name: &BoxName) -> String {
    format!("moor/{}", name.as_str().replace('/', "--"))
}

#[cfg(test)]
mod tests;
