//! The reads the pull request trigger makes with an installation token: a pull request and its
//! author's permission.

use moor_core::seam::clock::Ts;
use moor_core::seam::forge::ForgeRepo;
use moor_core::workflow::CommitSha;
use moor_core::Secret;
use serde::Deserialize;

use super::{bearer, AppClient};

/// The pull request fields moor decides on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pull {
    /// `open` or `closed`.
    pub(crate) open: bool,
    /// Whether it is a draft.
    pub(crate) draft: bool,
    /// The head commit.
    pub(crate) head_sha: CommitSha,
    /// The repo the head branch lives in; none when that fork was deleted.
    pub(crate) head_repo: Option<ForgeRepo>,
    /// The author's login.
    pub(crate) author: String,
    /// Whether the author is a bot account: of type `Bot`, or a login ending in `[bot]`.
    pub(crate) author_is_bot: bool,
}

/// A person's permission on a repo, as GitHub's `permission` field gives it. `maintain` reads
/// as `Write` there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Permission {
    /// `admin`.
    Admin,
    /// `write`, which also covers `maintain`.
    Write,
    /// `read`, which also covers `triage`.
    Read,
    /// `none`.
    None,
}

impl Permission {
    /// Whether this lets a fork's author fire a run.
    pub(crate) const fn can_write(self) -> bool {
        matches!(self, Self::Admin | Self::Write)
    }
}

impl AppClient {
    /// The head commit of pull request `number` on `repo`, read with a token of the installation
    /// that covers `repo`.
    ///
    /// # Errors
    /// The App is not installed on `repo`, GitHub cannot be reached, or the pull request is not
    /// there.
    pub(crate) fn head_of(
        &self,
        repo: &ForgeRepo,
        number: u64,
        now: Ts,
    ) -> Result<CommitSha, String> {
        let token = self.token_for(repo, now)?;
        Ok(self.pull(repo, number, &token)?.head_sha)
    }

    /// A read-only token of the installation that covers `repo`.
    ///
    /// # Errors
    /// The App is not installed on `repo`, or GitHub cannot be reached or refuses the mint.
    pub(crate) fn token_for(&self, repo: &ForgeRepo, now: Ts) -> Result<Secret, String> {
        let installation = self
            .installation_of(repo, now)?
            .ok_or_else(|| format!("the GitHub App is not installed on {repo}"))?;
        self.token(installation, now)
    }

    /// Pull request `number` on `repo`, read with an installation `token`.
    ///
    /// # Errors
    /// GitHub cannot be reached, the pull request is not there, or its head is not a full SHA.
    pub(crate) fn pull(
        &self,
        repo: &ForgeRepo,
        number: u64,
        token: &Secret,
    ) -> Result<Pull, String> {
        /// A user.
        #[derive(Deserialize)]
        struct User {
            /// Login.
            login: String,
            /// `User`, `Bot` or `Organization`.
            #[serde(rename = "type")]
            kind: String,
        }
        /// The head's repo.
        #[derive(Deserialize)]
        struct HeadRepo {
            /// `owner/name`.
            full_name: String,
        }
        /// The head.
        #[derive(Deserialize)]
        struct Head {
            /// The head commit.
            sha: String,
            /// None when the fork was deleted.
            repo: Option<HeadRepo>,
        }
        /// `GET /repos/{o}/{r}/pulls/{n}`.
        #[derive(Deserialize)]
        struct Raw {
            /// `open` or `closed`.
            state: String,
            /// Draft flag.
            #[serde(default)]
            draft: bool,
            /// The head.
            head: Head,
            /// The author.
            user: User,
        }
        let path = format!("/repos/{}/{}/pulls/{number}", repo.owner, repo.name);
        let raw: Raw = self.get(&path, &bearer(token)?)?;
        let head_sha = CommitSha::parse(&raw.head.sha)
            .map_err(|e| format!("GitHub answered {path} with head {}: {e}", raw.head.sha))?;
        Ok(Pull {
            open: raw.state == "open",
            draft: raw.draft,
            head_sha,
            head_repo: raw
                .head
                .repo
                .and_then(|r| ForgeRepo::parse(&r.full_name).ok()),
            author_is_bot: raw.user.kind == "Bot" || raw.user.login.ends_with("[bot]"),
            author: raw.user.login,
        })
    }

    /// `user`'s permission on `repo`, read with an installation `token`.
    ///
    /// # Errors
    /// GitHub cannot be reached, or answers a permission moor does not know.
    pub(crate) fn permission(
        &self,
        repo: &ForgeRepo,
        user: &str,
        token: &Secret,
    ) -> Result<Permission, String> {
        /// `GET /repos/{o}/{r}/collaborators/{user}/permission`.
        #[derive(Deserialize)]
        struct Raw {
            /// `admin`, `write`, `read` or `none`.
            permission: String,
        }
        let path = format!(
            "/repos/{}/{}/collaborators/{user}/permission",
            repo.owner, repo.name
        );
        let raw: Raw = self.get(&path, &bearer(token)?)?;
        match raw.permission.as_str() {
            "admin" => Ok(Permission::Admin),
            "write" | "maintain" => Ok(Permission::Write),
            "read" | "triage" => Ok(Permission::Read),
            "none" => Ok(Permission::None),
            _ => Err(format!("GitHub answered {path} with an unknown permission")),
        }
    }
}
