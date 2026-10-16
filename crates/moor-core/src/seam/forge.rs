//! Forge seam: the code host that names repos and mints repo tokens.

use std::fmt;

use crate::secret::Secret;

/// A repository on a forge, named by owner and repo name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ForgeRepo {
    /// Account or organisation that owns the repo.
    pub owner: String,
    /// Repo name, without the owner.
    pub name: String,
}

impl ForgeRepo {
    /// Builds a repo reference from an owner and a name.
    #[must_use]
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
        }
    }
}

impl fmt::Display for ForgeRepo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// Why a forge call failed.
///
/// The messages never contain a token; callers may log or show them as-is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeError {
    /// The forge rejected the credentials.
    Auth(String),
    /// The forge could not be reached.
    Network(String),
    /// The call failed for any other reason.
    Failed(String),
}

impl fmt::Display for ForgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth(m) => write!(f, "forge auth failed: {m}"),
            Self::Network(m) => write!(f, "forge network error: {m}"),
            Self::Failed(m) => write!(f, "forge call failed: {m}"),
        }
    }
}

impl std::error::Error for ForgeError {}

/// A code host, behind whichever implementation is wired.
pub trait Forge: Send + Sync {
    /// Maps a git origin URL to a forge repo, or `None` when the URL is not on this forge.
    fn repo_of(&self, origin: &str) -> Option<ForgeRepo>;

    /// Returns a token that can access `repo`.
    ///
    /// # Errors
    /// Returns a [`ForgeError`] when the forge refuses or cannot be reached.
    fn repo_token(&self, repo: &ForgeRepo) -> Result<Secret, ForgeError>;

    /// The web URL of the open pull request whose head is `branch` in `repo`, or `None` when there is none.
    ///
    /// # Errors
    /// Returns a [`ForgeError`] when the forge refuses or cannot be reached.
    fn open_pr(&self, repo: &ForgeRepo, branch: &str) -> Result<Option<String>, ForgeError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forge_repo_displays_owner_slash_name() {
        assert_eq!(
            ForgeRepo::new("acme", "widgets").to_string(),
            "acme/widgets"
        );
    }

    #[test]
    fn forge_repo_serde_shape() {
        let repo = ForgeRepo::new("acme", "widgets");
        let json = serde_json::to_string(&repo).unwrap();
        assert_eq!(json, r#"{"owner":"acme","name":"widgets"}"#);
        assert_eq!(serde_json::from_str::<ForgeRepo>(&json).unwrap(), repo);
    }

    #[test]
    fn forge_error_displays_each_variant() {
        assert_eq!(
            ForgeError::Auth("bad".into()).to_string(),
            "forge auth failed: bad"
        );
        assert_eq!(
            ForgeError::Network("down".into()).to_string(),
            "forge network error: down"
        );
        assert_eq!(
            ForgeError::Failed("boom".into()).to_string(),
            "forge call failed: boom"
        );
    }
}
