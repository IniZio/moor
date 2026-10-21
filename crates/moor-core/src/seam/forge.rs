//! Forge seam: the code host that names repos.

use std::fmt;

use crate::name::Sha;
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

    /// `https://github.com/OWNER/NAME.git`, the URL a Box clones and pushes; the Proxy's
    /// `github-git` rule allows this `.git` path.
    #[must_use]
    pub fn clone_url(&self) -> String {
        format!("https://github.com/{}/{}.git", self.owner, self.name)
    }
}

impl fmt::Display for ForgeRepo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// What the forge says about a repo at the commit a Box starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoAt {
    /// Full sha the base ref resolved to.
    pub base: Sha,
    /// Tip of the Box branch on the forge, when that branch exists.
    pub branch_tip: Option<Sha>,
    /// Config files read at `base`.
    pub files: RepoFiles,
    /// Commit identity of the token's account.
    pub author: GitAuthor,
    /// Whether the token's account may push to the repo.
    pub can_push: bool,
}

/// The repo files moor reads at a Box's base; `None` where the file is absent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoFiles {
    /// `.moor.yaml`.
    pub moor_yaml: Option<String>,
    /// `.mcp.json`.
    pub mcp_json: Option<String>,
    /// `.claude/settings.json`.
    pub claude_settings: Option<String>,
}

/// The name and email commits are made with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitAuthor {
    /// Display name.
    pub name: String,
    /// Email address.
    pub email: String,
}

/// Why a forge read failed; the text never carries the token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ForgeError {
    /// The repo does not exist or the token's account cannot read it.
    #[error("the repo is not visible to your account")]
    NotVisible,
    /// The base ref does not name a commit in the repo.
    #[error("base `{0}` does not name a commit in the repo")]
    UnknownBase(String),
    /// A config file is binary or too large to read.
    #[error("{0} is binary or too large to read")]
    Unreadable(&'static str),
    /// The forge refused the token.
    #[error("the forge rejected the token")]
    Rejected,
    /// The forge could not be reached or answered unexpectedly.
    #[error("the forge is unavailable: {0}")]
    Unavailable(String),
}

/// A code host, behind whichever implementation is wired.
pub trait Forge: Send + Sync {
    /// Maps a git origin URL to a forge repo, or `None` when the URL is not on this forge.
    fn repo_of(&self, origin: &str) -> Option<ForgeRepo>;

    /// Reads `repo` at `base` (the default branch when `None`) as the account of `token`, with
    /// the tip of `branch` when it exists.
    ///
    /// # Errors
    /// Returns the [`ForgeError`] that says why the repo, base or a config file cannot be read.
    fn read(
        &self,
        repo: &ForgeRepo,
        base: Option<&str>,
        branch: &str,
        token: &Secret,
    ) -> Result<RepoAt, ForgeError>;
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
    fn clone_url_is_the_plain_https_git_url() {
        assert_eq!(
            ForgeRepo::new("acme", "widgets").clone_url(),
            "https://github.com/acme/widgets.git"
        );
    }

    #[test]
    fn forge_repo_serde_shape() {
        let repo = ForgeRepo::new("acme", "widgets");
        let json = serde_json::to_string(&repo).unwrap();
        assert_eq!(json, r#"{"owner":"acme","name":"widgets"}"#);
        assert_eq!(serde_json::from_str::<ForgeRepo>(&json).unwrap(), repo);
    }
}
