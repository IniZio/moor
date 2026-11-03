//! Where a Box's setup runs.

use std::path::Path;

use moor_core::domain::decision::BoxRepo;
use moor_core::seam::driver::Acquired;
use moor_core::seam::forge::ForgeRepo;
use moor_core::{BoxHandle, BoxPath};

use super::layers::Layered;

/// Where setup runs: the freshly acquired Box, the repo it clones and where its setup log is kept.
pub(super) struct SetupTarget<'a> {
    /// The Box setup runs in.
    pub(super) handle: &'a BoxHandle,
    /// The provider that holds the Box.
    pub(super) provider: &'a str,
    /// Working directory in the Box.
    pub(super) workdir: &'a BoxPath,
    /// The repo the Box clones, with the author it commits as; `None` for an empty workdir.
    pub(super) repo: Option<&'a BoxRepo>,
    /// Host directory for the Box's setup log.
    pub(super) log_dir: &'a Path,
    /// The listed repo whose tree this is, named on its setup steps; `None` for the primary's.
    pub(super) tree_of: Option<&'a ForgeRepo>,
    /// The primary repo's setup layers, when its lines run in checkpointed layers.
    pub(super) layered: Option<&'a Layered>,
}

impl<'a> SetupTarget<'a> {
    /// The target for a freshly acquired Box.
    pub(super) const fn new(
        got: &'a Acquired,
        provider: &'a str,
        repo: Option<&'a BoxRepo>,
        log_dir: &'a Path,
        layered: Option<&'a Layered>,
    ) -> Self {
        Self {
            handle: &got.handle,
            provider,
            workdir: &got.workdir,
            repo,
            log_dir,
            tree_of: None,
            layered,
        }
    }
}
