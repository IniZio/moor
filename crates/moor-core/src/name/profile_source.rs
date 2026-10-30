//! The git repo a person keeps their agent profile in.

use std::fmt;

use super::ForgeRepo;

/// A person's agent profile repo and the ref to read it at. On the wire and on the command line
/// it is `https://github.com/OWNER/NAME[#REF]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProfileSource {
    /// The repo on github.com.
    pub repo: ForgeRepo,
    /// A branch, tag or sha, as given; the repo's default branch when absent.
    pub git_ref: Option<String>,
}

/// Text that is not a profile repo address; the text is never shown, as it may carry a token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a profile must be https://github.com/OWNER/NAME, optionally followed by #REF")]
pub struct ProfileSourceError;

impl ProfileSource {
    /// Parses `https://github.com/OWNER/NAME[.git][#REF]`.
    ///
    /// # Errors
    /// [`ProfileSourceError`] for another host or scheme, a path that is not owner and repo, or
    /// an empty ref or one with whitespace, control characters or a leading `-`.
    pub fn parse(text: &str) -> Result<Self, ProfileSourceError> {
        let (url, git_ref) = match text.split_once('#') {
            Some((url, r)) => (url, Some(r)),
            None => (text, None),
        };
        if !url.starts_with("https://github.com/") {
            return Err(ProfileSourceError);
        }
        let repo = ForgeRepo::from_github_url(url).ok_or(ProfileSourceError)?;
        if let Some(r) = git_ref {
            let plain = !r.is_empty()
                && !r.starts_with('-')
                && !r.chars().any(|c| c.is_whitespace() || c.is_control());
            if !plain {
                return Err(ProfileSourceError);
            }
        }
        Ok(Self {
            repo,
            git_ref: git_ref.map(str::to_owned),
        })
    }
}

impl fmt::Display for ProfileSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "https://github.com/{}", self.repo)?;
        if let Some(r) = &self.git_ref {
            write!(f, "#{r}")?;
        }
        Ok(())
    }
}

impl TryFrom<String> for ProfileSource {
    type Error = ProfileSourceError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}

impl From<ProfileSource> for String {
    fn from(source: ProfileSource) -> Self {
        source.to_string()
    }
}

#[cfg(test)]
mod tests;
