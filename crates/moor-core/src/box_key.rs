//! Every internal identifier of a Box, derived from its [`BoxId`].
//!
//! The Box name is for display and lookup only: slugs and directories
//! come from the id, so a name with `/` or of any legal length is safe.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

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

impl Serialize for ControllerId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ControllerId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Self::parse(&text).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "a controller id is eight lowercase hex chars, not {text:?}"
            ))
        })
    }
}

/// A pool Box's place: the enrolled machine that hosts it and the controller that names its lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolSeat {
    /// The enrolled machine id.
    pub machine: i64,
    /// The controller whose lease slugs the host keeps apart from other controllers'.
    pub controller: ControllerId,
    /// Which of the Box's hands is placed here; the first is 1.
    pub inc: i64,
}

/// The controller, Box and incarnation a pool lease slug names; `None` for any other text, so a
/// host's agent keeps arbitrary crabbox names out.
///
/// The slug is `moor-{controller}-{id}` for the first incarnation and
/// `moor-{controller}-{id}-h{inc}` for a later one.
#[must_use]
pub fn parse_pool_slug(slug: &str) -> Option<(ControllerId, BoxId, i64)> {
    let (controller, rest) = slug.strip_prefix("moor-")?.split_once('-')?;
    let (digits, inc) = match rest.split_once("-h") {
        Some((digits, inc)) => (digits, canonical(inc).filter(|inc| *inc > 1)?),
        None => (rest, 1),
    };
    let id = canonical(digits)?;
    Some((ControllerId::parse(controller)?, BoxId::new(id), inc))
}

/// The positive decimal number `text` spells with no sign, padding or suffix.
fn canonical(text: &str) -> Option<i64> {
    let n: i64 = text.parse().ok().filter(|n| *n > 0)?;
    (n.to_string() == text).then_some(n)
}

/// The machine and slug a [`BoxKey::machine_handle`] names; `None` for any other text, so a
/// handle of another driver is never taken for one.
#[must_use]
pub fn parse_machine_handle(handle: &str) -> Option<(i64, &str)> {
    let (machine, slug) = handle.strip_prefix('m')?.split_once('/')?;
    let machine = canonical(machine)?;
    parse_pool_slug(slug).map(|_| (machine, slug))
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

    /// The lease slug of hands `inc` of a Box on a user's pool host.
    ///
    /// It is `moor-{controller}-{id}` for the first and `moor-{controller}-{id}-h{inc}` for a
    /// later one. Box ids repeat across controllers that share a host, so the controller id keeps
    /// their leases apart; the incarnation keeps a lost VM's report apart from its replacement's.
    #[must_use]
    pub fn pool_slug(self, controller: &ControllerId, inc: i64) -> String {
        let (controller, id) = (controller.as_str(), self.0.get());
        if inc > 1 {
            format!("moor-{controller}-{id}-h{inc}")
        } else {
            format!("moor-{controller}-{id}")
        }
    }

    /// The handle of the Box on a machine whose agent provides Boxes itself: `m{machine}/` and
    /// the lease slug of `seat`'s hands. [`parse_machine_handle`] reads it back.
    #[must_use]
    pub fn machine_handle(self, seat: &PoolSeat) -> String {
        format!(
            "m{}/{}",
            seat.machine,
            self.pool_slug(&seat.controller, seat.inc)
        )
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

/// The git branch of Box `id` named `name`: `moor/{id}-{repo}--{label}`.
///
/// The id leads, so a name reused after its Box closed never lands on the old Box's branch. The
/// `/` of the name becomes `--`, so the branch holds exactly one `/`.
#[must_use]
pub fn branch(id: BoxId, name: &BoxName) -> String {
    format!("moor/{}-{}", id.get(), name.as_str().replace('/', "--"))
}

#[cfg(test)]
mod tests;
