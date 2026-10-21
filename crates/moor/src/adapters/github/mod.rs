//! GitHub forge adapter: repo naming comes from origin URL parsing.

mod origin;
#[cfg(test)]
mod tests;

use moor_core::seam::forge::{Forge, ForgeRepo};

/// GitHub as a [`Forge`].
pub(crate) struct GitHub;

impl Forge for GitHub {
    fn repo_of(&self, origin: &str) -> Option<ForgeRepo> {
        origin::parse_origin(origin)
    }
}
