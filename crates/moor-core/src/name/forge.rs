//! A repo on github.com and the commit identity of a GitHub account.

use std::fmt;

/// A repository on github.com, named by owner and repo name; on the wire it is `OWNER/NAME`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
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

    /// Parses `OWNER/NAME`: one `/`, each segment `[A-Za-z0-9._-]+` and neither `.` nor `..`,
    /// the owner at most 39 bytes and the name at most 100, as GitHub allows.
    ///
    /// # Errors
    /// [`RepoNameError`] for any other text.
    pub fn parse(s: &str) -> Result<Self, RepoNameError> {
        let segment = |p: &str, max: usize| {
            (1..=max).contains(&p.len())
                && p != "."
                && p != ".."
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        };
        match s.split_once('/') {
            Some((owner, name)) if segment(owner, OWNER_CAP) && segment(name, NAME_CAP) => {
                Ok(Self::new(owner, name))
            }
            _ => Err(RepoNameError),
        }
    }

    /// The repo a git origin URL on github.com names: `https://`, `ssh://` or
    /// `git@github.com:OWNER/NAME`, with an optional `.git` and one trailing slash. `None` for any
    /// other host, scheme, port or path shape.
    #[must_use]
    pub fn from_github_url(origin: &str) -> Option<Self> {
        parse_origin(origin)
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

/// Longest GitHub account or organisation name.
const OWNER_CAP: usize = 39;

/// Longest GitHub repo name.
const NAME_CAP: usize = 100;

/// Text that is not `OWNER/NAME`; the text is never shown, as it may be a URL with a token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a repo must be OWNER/NAME")]
pub struct RepoNameError;

impl TryFrom<String> for ForgeRepo {
    type Error = RepoNameError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}

impl From<ForgeRepo> for String {
    fn from(repo: ForgeRepo) -> Self {
        repo.to_string()
    }
}

/// The name and email commits are made with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitAuthor {
    /// Display name.
    pub name: String,
    /// Email address.
    pub email: String,
}

/// The only host whose origins are accepted; GitHub Enterprise hosts are not.
const HOST: &str = "github.com";

/// Parses a git origin URL into its owner and repo name.
///
/// Accepts `https://` and `ssh://` URLs and the scp form `git@github.com:owner/name`, with an
/// optional `.git` suffix and one trailing slash. Any other host, scheme, port, or path shape is
/// refused.
fn parse_origin(origin: &str) -> Option<ForgeRepo> {
    let (authority, path) = if let Some(rest) = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("ssh://"))
    {
        rest.split_once('/')?
    } else if origin.contains("://") {
        return None;
    } else {
        origin.split_once(':')?
    };
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if !host.eq_ignore_ascii_case(HOST) {
        return None;
    }
    repo_from_path(path)
}

/// Splits `owner/name[.git][/]` into a repo; exactly two valid segments are required.
fn repo_from_path(path: &str) -> Option<ForgeRepo> {
    let path = path.strip_suffix('/').unwrap_or(path);
    let path = path.strip_suffix(".git").unwrap_or(path);
    ForgeRepo::parse(path).ok()
}

#[cfg(test)]
mod tests;
