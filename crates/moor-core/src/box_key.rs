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

/// A controller's short id, eight lowercase hex chars, made once per store.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ControllerId(String);

impl ControllerId {
    /// The id spelled by `bytes`.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 4]) -> Self {
        Self(format!("{:08x}", u32::from_be_bytes(bytes)))
    }

    /// The id written as `text`; `None` for anything but eight lowercase hex chars.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.len() == 8 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        hex.then(|| Self(text.to_owned()))
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A pool Box's place: the enrolled machine that hosts it and the controller that names its lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolSeat {
    /// The enrolled machine id.
    pub machine: i64,
    /// The controller whose lease slugs the host keeps apart from other controllers'.
    pub controller: ControllerId,
}

/// The controller and Box a pool lease slug `moor-{controller}-{id}` names; `None` for any other
/// text, so a host's agent keeps arbitrary crabbox names out.
#[must_use]
pub fn parse_pool_slug(slug: &str) -> Option<(ControllerId, BoxId)> {
    let (controller, digits) = slug.strip_prefix("moor-")?.split_once('-')?;
    let id: i64 = digits.parse().ok().filter(|id| *id > 0)?;
    (id.to_string() == digits).then_some((ControllerId::parse(controller)?, BoxId::new(id)))
}

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

    /// The lease slug of a Box on a user's pool host, `moor-{controller}-{id}`. Box ids repeat
    /// across controllers that share a host, so the controller id keeps their leases apart.
    #[must_use]
    pub fn pool_slug(self, controller: &ControllerId) -> String {
        format!("moor-{}-{}", controller.as_str(), self.0.get())
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
