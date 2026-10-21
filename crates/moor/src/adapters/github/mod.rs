//! GitHub forge adapter: repo naming comes from origin URL parsing, repo reads from GraphQL.

mod origin;
mod read;
#[cfg(test)]
mod tests;

use moor_core::seam::forge::{Forge, ForgeError, ForgeRepo, RepoAt};
use moor_core::Secret;

/// GitHub as a [`Forge`].
pub(crate) struct GitHub {
    /// GraphQL endpoint reads are posted to.
    api: String,
}

impl GitHub {
    /// GitHub at its public API.
    pub(crate) fn new() -> Self {
        Self {
            api: read::API.to_owned(),
        }
    }

    /// GitHub with reads posted to `api`.
    #[cfg(test)]
    fn at(api: impl Into<String>) -> Self {
        Self { api: api.into() }
    }
}

impl Forge for GitHub {
    fn repo_of(&self, origin: &str) -> Option<ForgeRepo> {
        origin::parse_origin(origin)
    }

    fn read(
        &self,
        repo: &ForgeRepo,
        base: Option<&str>,
        branch: &str,
        token: &Secret,
    ) -> Result<RepoAt, ForgeError> {
        read::read(&self.api, repo, base, branch, token)
    }
}
