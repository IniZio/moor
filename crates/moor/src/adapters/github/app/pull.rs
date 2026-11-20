//! The reads the pull request trigger makes with an installation token: a pull request and its
//! author's permission.

use moor_core::seam::forge::ForgeRepo;
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
    pub(crate) head_sha: String,
    /// The repo the head branch lives in; none when that fork was deleted.
    pub(crate) head_repo: Option<ForgeRepo>,
    /// The author's login.
    pub(crate) author: String,
    /// Whether the author is a bot account.
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
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the pull request trigger reads it")
    )]
    pub(crate) const fn can_write(self) -> bool {
        matches!(self, Self::Admin | Self::Write)
    }
}

impl AppClient {
    /// Pull request `number` on `repo`, read with an installation `token`.
    ///
    /// # Errors
    /// GitHub cannot be reached, or the pull request is not there.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the pull request trigger reads it")
    )]
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
        Ok(Pull {
            open: raw.state == "open",
            draft: raw.draft,
            head_sha: raw.head.sha,
            head_repo: raw
                .head
                .repo
                .and_then(|r| ForgeRepo::parse(&r.full_name).ok()),
            author_is_bot: raw.user.kind == "Bot",
            author: raw.user.login,
        })
    }

    /// `user`'s permission on `repo`, read with an installation `token`.
    ///
    /// # Errors
    /// GitHub cannot be reached, or answers a permission moor does not know.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the pull request trigger reads it")
    )]
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
