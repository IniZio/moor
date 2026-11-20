//! Why a run started, and the pull request it is for when it has one.

use std::fmt;

use super::RunTrigger;
use crate::name::ForgeRepo;

/// A commit's full SHA: 40 hex characters, stored lowercase.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CommitSha(String);

impl CommitSha {
    /// Parses a full commit SHA and lowercases it.
    ///
    /// # Errors
    /// [`PullError::Sha`] unless `s` is exactly 40 hex characters.
    pub fn parse(s: &str) -> Result<Self, PullError> {
        if s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
            Ok(Self(s.to_ascii_lowercase()))
        } else {
            Err(PullError::Sha(s.to_owned()))
        }
    }

    /// The SHA as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first seven characters, as GitHub abbreviates a commit.
    #[must_use]
    pub fn short(&self) -> &str {
        self.0.get(..7).unwrap_or(&self.0)
    }
}

impl fmt::Display for CommitSha {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for CommitSha {
    type Error = PullError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}

impl From<CommitSha> for String {
    fn from(sha: CommitSha) -> Self {
        sha.0
    }
}

/// Text that names no pull request or commit.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PullError {
    /// Not `OWNER/NAME#NUMBER`.
    #[error("invalid pull request `{0}`: write OWNER/NAME#NUMBER, such as IniZio/moor#12")]
    Target(String),
    /// Not 40 hex characters.
    #[error("invalid commit SHA `{0}`: a full SHA is 40 hex characters")]
    Sha(String),
}

/// A pull request a caller names, before its head is read: `OWNER/NAME#NUMBER` on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PullTarget {
    /// The repo the pull request is on.
    pub repo: ForgeRepo,
    /// Its number, from 1.
    pub number: u64,
}

impl PullTarget {
    /// Parses `OWNER/NAME#NUMBER`.
    ///
    /// # Errors
    /// [`PullError::Target`] for any other text.
    pub fn parse(s: &str) -> Result<Self, PullError> {
        let bad = || PullError::Target(s.to_owned());
        let (repo, number) = s.trim().split_once('#').ok_or_else(bad)?;
        let number = number
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0 && number.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(bad)?;
        Ok(Self {
            repo: ForgeRepo::parse(repo).map_err(|_| bad())?,
            number,
        })
    }

    /// This pull request at commit `head_sha`.
    #[must_use]
    pub fn at(self, head_sha: CommitSha) -> PullRef {
        PullRef {
            repo: self.repo,
            number: self.number,
            head_sha,
        }
    }
}

impl fmt::Display for PullTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.repo, self.number)
    }
}

impl TryFrom<String> for PullTarget {
    type Error = PullError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}

impl From<PullTarget> for String {
    fn from(t: PullTarget) -> Self {
        t.to_string()
    }
}

/// One pull request at one head commit: what a run for a pull request works on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PullRef {
    /// The repo, which is the workflow's first repo.
    pub repo: ForgeRepo,
    /// The pull request's number.
    pub number: u64,
    /// The head commit the run is for.
    pub head_sha: CommitSha,
}

impl PullRef {
    /// The pull request's page on github.com.
    #[must_use]
    pub fn url(&self) -> String {
        format!(
            "https://github.com/{}/{}/pull/{}",
            self.repo.owner, self.repo.name, self.number
        )
    }

    /// The variables a run's Box gets for this pull request, by name.
    #[must_use]
    pub fn env(&self) -> [(&'static str, String); 4] {
        [
            ("MOOR_PULL_REPO", self.repo.to_string()),
            ("MOOR_PULL_NUMBER", self.number.to_string()),
            ("MOOR_PULL_HEAD_SHA", self.head_sha.to_string()),
            ("MOOR_PULL_URL", self.url()),
        ]
    }
}

impl PullRef {
    /// Whether this is pull request `target`, at any head. Repo names compare as GitHub does,
    /// without case.
    #[must_use]
    pub fn is(&self, target: &PullTarget) -> bool {
        self.number == target.number
            && self.repo.owner.eq_ignore_ascii_case(&target.repo.owner)
            && self.repo.name.eq_ignore_ascii_case(&target.repo.name)
    }
}

impl fmt::Display for PullRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}#{} at {}",
            self.repo,
            self.number,
            self.head_sha.short()
        )
    }
}

/// Why a run started. One enum, so a cron run never has a pull request and a pull request event
/// never fires a run without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunCause {
    /// The workflow's schedule came due.
    Cron,
    /// Its owner asked for one run now, with `run_workflow` or Run again; `pull` is set for a
    /// pull request.
    Manual {
        /// The pull request at its head when the run was asked for.
        pull: Option<PullRef>,
    },
    /// A pull request event, at the head read back from GitHub.
    PullRequest {
        /// The pull request.
        pull: PullRef,
    },
}

impl RunCause {
    /// The trigger the store and the lists name.
    #[must_use]
    pub const fn trigger(&self) -> RunTrigger {
        match self {
            Self::Cron => RunTrigger::Cron,
            Self::Manual { .. } => RunTrigger::Manual,
            Self::PullRequest { .. } => RunTrigger::PullRequest,
        }
    }

    /// The pull request the run is for, when it is for one.
    #[must_use]
    pub const fn subject(&self) -> Option<&PullRef> {
        match self {
            Self::Cron | Self::Manual { pull: None } => None,
            Self::Manual { pull: Some(pull) } | Self::PullRequest { pull } => Some(pull),
        }
    }

    /// Whether this run and `other` are about the same subject: both about no pull request, or
    /// both about the same pull request at any head. One subject holds one open run.
    #[must_use]
    pub fn same_subject(&self, other: &Self) -> bool {
        match (self.subject(), other.subject()) {
            (None, None) => true,
            (Some(a), Some(b)) => b.is(&PullTarget {
                repo: a.repo.clone(),
                number: a.number,
            }),
            _ => false,
        }
    }

    /// The cause a stored trigger and subject stand for; none when they do not fit together, such
    /// as a cron run with a pull request.
    #[must_use]
    pub fn from_parts(trigger: RunTrigger, pull: Option<PullRef>) -> Option<Self> {
        match (trigger, pull) {
            (RunTrigger::Cron, None) => Some(Self::Cron),
            (RunTrigger::Manual, pull) => Some(Self::Manual { pull }),
            (RunTrigger::PullRequest, Some(pull)) => Some(Self::PullRequest { pull }),
            (RunTrigger::Cron, Some(_)) | (RunTrigger::PullRequest, None) => None,
        }
    }
}

#[cfg(test)]
mod tests;
