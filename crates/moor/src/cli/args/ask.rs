//! clap definitions for `moor ask`.

use clap::Args;
use moor_core::request::AskReq;
use moor_core::seam::forge::ForgeRepo;

/// Arguments of `ask`.
#[derive(Debug, Args)]
pub(super) struct AskArgs {
    /// A repo the task is about, as OWNER/NAME or a github.com URL; repeat for more, the task's
    /// own first.
    #[arg(
        long,
        required = true,
        value_name = "OWNER/NAME|URL",
        value_parser = super::super::caller::repo_arg
    )]
    repo: Vec<ForgeRepo>,
    /// What the agent should do or answer.
    prompt: String,
}

impl From<AskArgs> for AskReq {
    fn from(a: AskArgs) -> Self {
        Self {
            repos: a.repo,
            prompt: a.prompt,
        }
    }
}
