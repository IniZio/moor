//! Where artifacts, reports and transcripts live: named by key, kept by one backend.

use std::fmt;
use std::io;
use std::path::Path;

use moor_core::BoxId;

pub(crate) mod fs;

/// Where one blob lives, the same under every backend.
///
/// Every part comes from the store's own ids or an already validated name. A Box id is unique
/// across users and a reader checks access on the row first, so no key names a user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlobKey(String);

impl BlobKey {
    /// Version `version` of the Box's artifact `name`.
    pub(crate) fn artifact(box_id: BoxId, version: i64, name: &str) -> Self {
        Self(format!("artifacts/{}/v{version}/{name}", box_id.get()))
    }

    /// The Box's artifact manifest.
    pub(crate) fn artifact_manifest(box_id: BoxId) -> Self {
        Self(format!("artifacts/{}/artifact-manifest.json", box_id.get()))
    }

    /// A key spelled out, for a test that lays out its own files.
    #[cfg(test)]
    pub(crate) fn raw(key: &str) -> Self {
        Self(key.to_owned())
    }

    /// The key's text.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a blob operation failed. Messages never carry blob contents.
#[derive(Debug)]
pub(crate) enum BlobError {
    /// No blob under that key.
    Missing,
    /// A backend refused the key's path as unsafe to follow.
    NotPlain(String),
    /// The store could not be reached or refused the operation.
    Store(String),
    /// The receiver of a read refused a piece.
    Sink(io::Error),
}

impl fmt::Display for BlobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("no such blob"),
            Self::NotPlain(why) | Self::Store(why) => f.write_str(why),
            Self::Sink(e) => write!(f, "stopped sending: {}", e.kind()),
        }
    }
}

impl From<BlobError> for io::Error {
    fn from(e: BlobError) -> Self {
        match e {
            BlobError::Missing => Self::from(io::ErrorKind::NotFound),
            BlobError::NotPlain(why) => Self::new(io::ErrorKind::InvalidInput, why),
            BlobError::Store(why) => Self::other(why),
            BlobError::Sink(e) => e,
        }
    }
}

/// One store of blobs. Calls block; they must not run on a tokio worker.
pub(crate) trait Blobs: Send + Sync {
    /// Stores `bytes` under `key`, replacing what is there.
    fn put(&self, key: &BlobKey, bytes: &[u8]) -> Result<(), BlobError>;

    /// Stores the regular file at `from` under `key`, replacing what is there; `from` is gone
    /// after success. A key is only recorded once this returns, so replacing is always safe.
    fn put_file(&self, key: &BlobKey, from: &Path) -> Result<(), BlobError>;

    /// The size of the blob, in bytes.
    fn size(&self, key: &BlobKey) -> Result<u64, BlobError>;

    /// Passes `len` bytes from `offset` to `sink`, in pieces of any size, the next only after
    /// `sink` took the last. The caller checks the range against [`Self::size`].
    fn read(
        &self,
        key: &BlobKey,
        offset: u64,
        len: u64,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<(), BlobError>;

    /// What to show a person for `key`: never read back to reach the blob.
    fn locate(&self, key: &BlobKey) -> String;

    /// The backend's name in a manifest.
    fn name(&self) -> &'static str;
}

#[cfg(test)]
pub(crate) mod tests;
