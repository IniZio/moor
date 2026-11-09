//! Where a saved transcript goes and how it is told apart: pure naming and digesting.

use std::fmt::Write as _;
use std::io::{self, Read};

use sha2::{Digest, Sha256};

use crate::name::BoxId;

/// What ended the session whose transcript is saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The agent was started again.
    Relaunch,
    /// The Box was closed.
    Close,
    /// The session was ended on request, leaving the Box open.
    End,
    /// The agent was first seen to have exited.
    Exit,
    /// The agent finished a turn.
    Turn,
}

impl Trigger {
    /// The name recorded in events.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Relaunch => "relaunch",
            Self::Close => "close",
            Self::End => "end",
            Self::Exit => "exit",
            Self::Turn => "turn",
        }
    }
}

/// The archive's blob key: `transcripts/<box>/<session>/<digest>.tgz`.
///
/// The key names the content, so saving an unchanged archive again puts the same blob.
#[must_use]
pub fn relative_path(box_id: BoxId, session: i64, digest: &str) -> String {
    format!("transcripts/{}/{session}/{digest}.tgz", box_id.get())
}

/// Whether an archive with `digest` repeats the one already stored for the session.
#[must_use]
pub fn is_duplicate(stored: Option<&str>, digest: &str) -> bool {
    stored == Some(digest)
}

/// Lowercase hex sha256 of the archive, read as a stream so its size does not matter.
///
/// # Errors
/// Returns the read error.
pub fn digest(mut archive: impl Read) -> io::Result<String> {
    let mut hasher = Sha256::new();
    io::copy(&mut archive, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut acc, b| {
            let _ = write!(acc, "{b:02x}");
            acc
        }))
}

#[cfg(test)]
mod tests;
