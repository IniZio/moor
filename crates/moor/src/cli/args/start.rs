//! clap definitions for `moor start`.

use clap::Args;
use moor_core::config::override_text;
use moor_core::decide::schedule::Placement;
use moor_core::{BoxName, SessionId, StartReq};

use super::box_name;

/// Arguments of `start`.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools, reason = "one field per CLI flag")]
pub(super) struct StartArgs {
    /// Box name.
    #[arg(value_parser = box_name)]
    name: BoxName,
    /// Prompt for the agent.
    #[arg(long)]
    prompt: Option<String>,
    /// Issue reference.
    #[arg(long)]
    issue: Option<String>,
    /// Base ref.
    #[arg(long)]
    base: Option<String>,
    /// Repo to clone, as OWNER/NAME or a github.com URL; the current checkout's origin when absent.
    #[arg(long, value_name = "OWNER/NAME|URL", value_parser = super::super::caller::repo_arg)]
    repo: Option<moor_core::seam::forge::ForgeRepo>,
    /// Another repo to clone beside `--repo`, as OWNER/NAME or a github.com URL; repeat for
    /// more. Any use replaces `box.also` from your config.
    #[arg(long, value_name = "OWNER/NAME|URL", value_parser = super::super::caller::repo_arg)]
    also: Vec<moor_core::seam::forge::ForgeRepo>,
    /// Agent model.
    #[arg(long)]
    model: Option<String>,
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
    /// Do not launch an agent.
    #[arg(long)]
    no_agent: bool,
    /// Resume the agent's stored session.
    #[arg(long)]
    resume: bool,
    /// Parent Session id; defaults to the `MOOR_SESSION` of the calling agent.
    #[arg(long, value_name = "ID", env = "MOOR_SESSION")]
    parent: Option<i64>,
}

impl From<StartArgs> for StartReq {
    fn from(a: StartArgs) -> Self {
        Self {
            cold: false,
            placement: Placement::FirstOnly,
            name: a.name,
            prompt: a.prompt,
            issue: a.issue,
            base: a.base,
            repo: a.repo,
            model: a.model,
            node: a.node,
            config: override_text(a.config),
            no_agent: a.no_agent,
            resume: a.resume,
            parent: a.parent.map(SessionId::new),
            also: (!a.also.is_empty()).then_some(a.also),
            identity: None,
        }
    }
}
