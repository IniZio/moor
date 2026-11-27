//! The arguments the gate's MCP tools take.

use schemars::JsonSchema;
use serde::Deserialize;

/// Arguments of `start_box`. A field it does not know, such as a `prompt`, is refused, since a
/// task with words starts through `ask`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct StartArgs {
    /// The repos to clone, each as `owner/name`, first one first. The Box's branch and pull
    /// request go to the first; the rest are cloned beside it, and each must be one you can push
    /// to. Naming only one adds your configured `box.also` beside it.
    pub(super) repos: Vec<String>,
    /// A name for the Box; one is made up when left out.
    pub(super) name: Option<String>,
    /// The revision to branch from; the repo's default branch when left out.
    pub(super) base: Option<String>,
    /// A node `list_nodes` shows you; omit to start on the node `list_nodes` says a start picks.
    pub(super) node: Option<String>,
    /// A partial repo config, as YAML in the repo config format, laid over the repo's stored
    /// config for this Box only: a key you set wins, `env` merges by name, lists replace.
    /// `proxy.allow` adds extra `METHOD /path` entries, each with `{repo}`, to a default Proxy
    /// rule for this Box only.
    pub(super) config: Option<String>,
    /// Skip the repo's setup lines, on this machine and any later one: the Box is ready once the
    /// repo is cloned. False by default.
    pub(super) no_setup: Option<bool>,
    /// Wait for setup to finish before returning; false by default, so the call returns once the
    /// Box exists and `box_status` shows its progress.
    pub(super) wait: Option<bool>,
}

/// Arguments of `ask`.
#[derive(Deserialize, JsonSchema)]
pub(super) struct AskArgs {
    /// The repos the task is about, each as `owner/name`, first one first: the task's own.
    pub(super) repos: Vec<String>,
    /// What the agent should do or answer, as you would put it to the person's agent.
    pub(super) prompt: String,
}

/// Arguments of the tools that name one Box.
#[derive(Deserialize, JsonSchema)]
pub(super) struct BoxArgs {
    /// The Box's name, as `list_boxes` shows it.
    pub(super) r#box: String,
}

/// Arguments of `close_box`.
#[derive(Deserialize, JsonSchema)]
pub(super) struct CloseArgs {
    /// The Box's name, as `list_boxes` shows it.
    pub(super) r#box: String,
    /// Close even when the Box holds unpushed commits or uncommitted files, or could not be
    /// checked for them, discarding that work. False by default, like the CLI's `--force`.
    pub(super) force: Option<bool>,
}

/// Arguments of `send_to_box`.
#[derive(Deserialize, JsonSchema)]
pub(super) struct SendArgs {
    /// The Box's name.
    pub(super) r#box: String,
    /// The text to type to the Box's agent.
    pub(super) text: String,
}
