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
    /// Start a place: a Box with no repo that the agent waits in until a task claims it. It
    /// stays out of every list until then. Only a user on the controller's `agent_places` list
    /// may start one.
    #[arg(
        long,
        conflicts_with_all = ["prompt", "issue", "base", "repo", "config", "no_agent", "resume"]
    )]
    place: bool,
    /// Prompt for the agent.
    #[arg(long)]
    prompt: Option<String>,
    /// Issue reference.
    #[arg(long)]
    issue: Option<String>,
    /// Base ref.
    #[arg(long)]
    base: Option<String>,
    /// A repo to clone, as OWNER/NAME or a github.com URL; repeat for more, first one first. The
    /// agent works in the first, and the rest sit beside it. The current checkout's origin when
    /// absent. Naming two or more replaces `box.also` from your config.
    #[arg(long, value_name = "OWNER/NAME|URL", value_parser = super::super::caller::repo_arg)]
    repo: Vec<moor_core::seam::forge::ForgeRepo>,
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
        let mut repos = a.repo.into_iter();
        let repo = repos.next();
        let also: Vec<_> = repos.collect();
        Self {
            gates: None,
            cold: false,
            placement: Placement::FirstOnly,
            place: a.place,
            name: a.name,
            prompt: a.prompt,
            issue: a.issue,
            base: a.base,
            repo,
            model: a.model,
            node: a.node,
            config: override_text(a.config),
            no_agent: a.no_agent,
            resume: a.resume,
            parent: a.parent.map(SessionId::new),
            also: (!also.is_empty()).then_some(also),
            identity: None,
        }
    }
}
