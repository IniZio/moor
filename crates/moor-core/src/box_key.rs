//! Every internal identifier of a Box, derived from its [`BoxId`].
//!
//! The Box name is for display and lookup only: slugs, directories and proof
//! paths come from the id, so a name with `/` or of any legal length is safe.

use std::path::PathBuf;

use crate::name::{BoxId, BoxName};

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

    /// The Box id a slug made by [`BoxKey::slug`] carries; `None` for any other slug.
    #[must_use]
    pub fn id_from_slug(slug: &str) -> Option<BoxId> {
        let (id, nonce) = slug.strip_prefix("moor-")?.split_once('-')?;
        let hex = !nonce.is_empty()
            && nonce
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) || !hex {
            return None;
        }
        id.parse().ok().map(BoxId::new)
    }

    /// The session ref recorded when the Engine reports none, `moor-{id}`.
    #[must_use]
    pub fn session_ref(self) -> String {
        format!("moor-{}", self.0.get())
    }

    /// The credential directory under the Box home, `.moor-cred-{id}-{nonce}`.
    #[must_use]
    pub fn cred_dir(self, nonce: &str) -> String {
        format!(".moor-cred-{}-{nonce}", self.0.get())
    }

    /// The scratch directory under the Box home, `.moor-tmp-{id}-{nonce}`.
    #[must_use]
    pub fn tmp_dir(self, nonce: &str) -> String {
        format!(".moor-tmp-{}-{nonce}", self.0.get())
    }

    /// Where the host keeps this Box's proof copies, relative to the state dir: `proof/{id}`.
    #[must_use]
    pub fn proof_dir(self) -> PathBuf {
        PathBuf::from("proof").join(self.0.get().to_string())
    }

    /// Where the host keeps the proof copy `file`, relative to the state dir.
    #[must_use]
    pub fn proof_file(self, file: &str) -> PathBuf {
        self.proof_dir().join(file)
    }

    /// The branch for this Box when [`branch`] is already taken: `{branch}-{id}`.
    #[must_use]
    pub fn branch_on_collision(self, name: &BoxName) -> String {
        format!("{}-{}", branch(name), self.0.get())
    }
}

/// The git branch of a new Box named `name`: `moor/{repo}-{n}`, with every `/`
/// in the name flattened to `-` so the branch holds exactly one `/`.
#[must_use]
pub fn branch(name: &BoxName) -> String {
    format!("moor/{}", name.as_str().replace('/', "-"))
}

#[cfg(test)]
mod tests;
