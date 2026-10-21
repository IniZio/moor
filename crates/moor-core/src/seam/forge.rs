//! Forge seam: the code host that names repos.

use std::fmt;

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

/// A code host, behind whichever implementation is wired.
pub trait Forge: Send + Sync {
    /// Maps a git origin URL to a forge repo, or `None` when the URL is not on this forge.
    fn repo_of(&self, origin: &str) -> Option<ForgeRepo>;
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
