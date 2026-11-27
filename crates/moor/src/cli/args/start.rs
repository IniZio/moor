//! clap definitions for `moor start`.

use clap::Args;
use moor_core::config::override_text;
use moor_core::decide::schedule::Placement;
use moor_core::{BoxName, SessionId, StartReq};

use super::box_name;

/// Arguments of `start`.
#[derive(Debug, Args)]
pub(super) struct StartArgs {
    /// Box name.
    #[arg(value_parser = box_name)]
    name: BoxName,
    /// Base ref.
    #[arg(long)]
    base: Option<String>,
    /// A repo to clone, as OWNER/NAME or a github.com URL; repeat for more, first one first. The
    /// Box's branch is on the first, and the rest sit beside it. The current checkout's origin when
    /// absent. Naming two or more replaces `box.also` from your config.
    #[arg(long, value_name = "OWNER/NAME|URL", value_parser = super::super::caller::repo_arg)]
    repo: Vec<moor_core::seam::forge::ForgeRepo>,
    /// A node of yours to run the Box on, in place of your default; see `moor node ls`.
    #[arg(long, value_name = "NODE")]
    node: Option<String>,
    /// A partial repo config, in the repo config format, laid over the repo's config for this
    /// Box only; `-` reads stdin.
    #[arg(
        long,
        value_name = "FILE",
        value_parser = super::config_text
    )]
    config: Option<String>,
    /// Skip the repo's setup lines: the Box is ready once the repo is cloned. A later machine of
    /// the Box skips them too.
    #[arg(long)]
    no_setup: bool,
    /// Parent Session id; defaults to the `MOOR_SESSION` of the calling agent.
    #[arg(long, value_name = "ID", env = "MOOR_SESSION")]
    parent: Option<i64>,
}

impl From<StartArgs> for StartReq {
    fn from(a: StartArgs) -> Self {
        let mut repos = a.repo.into_iter();
        let repo = repos.next();
        let also: Vec<_> = repos.collect();
        Self {
            gates: None,
            cold: false,
            placement: Placement::FirstOnly,
            name: a.name,
            prompt: None,
            issue: None,
            base: a.base,
            repo,
            model: None,
            node: a.node,
            config: override_text(a.config),
            no_agent: true,
            no_setup: a.no_setup,
            resume: false,
            parent: a.parent.map(SessionId::new),
            also: (!also.is_empty()).then_some(also),
            identity: None,
            task: None,
            run: false,
        }
    }
}
