//! GitHub forge adapter: repo naming comes from origin URL parsing, repo reads from GraphQL.

mod read;
#[cfg(test)]
mod tests;

use std::time::Duration;

use moor_core::seam::forge::{Forge, ForgeError, ForgeRepo, RepoAt};
use moor_core::{Secret, Sha};

/// GitHub as a [`Forge`].
pub(crate) struct GitHub {
    /// GraphQL endpoint reads are posted to.
    api: String,
    /// How long one read may take.
    timeout: Duration,
}

impl GitHub {
    /// GitHub at its public API.
    pub(crate) fn new() -> Self {
        Self {
            api: read::API.to_owned(),
            timeout: read::TIMEOUT,
        }
    }

    /// GitHub with reads posted to `api`.
    #[cfg(test)]
    fn at(api: impl Into<String>) -> Self {
        Self {
            api: api.into(),
            timeout: read::TIMEOUT,
        }
    }
}

impl Forge for GitHub {
    fn repo_of(&self, origin: &str) -> Option<ForgeRepo> {
        ForgeRepo::from_github_url(origin)
    }

    fn read(
        &self,
        repo: &ForgeRepo,
        base: Option<&str>,
        branch: &str,
        token: &Secret,
    ) -> Result<RepoAt, ForgeError> {
        read::read(&self.api, self.timeout, repo, base, branch, token)
    }

    fn read_file_at(
        &self,
        repo: &ForgeRepo,
        at: Option<&str>,
        path: &'static str,
        token: &Secret,
    ) -> Result<(Sha, Option<String>), ForgeError> {
        read::read_file_at(&self.api, self.timeout, repo, at, path, token)
    }
}
