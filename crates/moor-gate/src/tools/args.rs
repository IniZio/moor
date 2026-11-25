//! The arguments the gate's MCP tools take.

use schemars::JsonSchema;
use serde::Deserialize;

/// Arguments of `start_box`.
#[derive(Deserialize, JsonSchema)]
pub(super) struct StartArgs {
    /// The repos to clone, each as `owner/name`, first one first. The agent works in the first,
    /// where its branch and pull request go; the rest are cloned beside it, and each must be one
    /// you can push to. Naming only one adds your configured `box.also` beside it.
    pub(super) repos: Vec<String>,
    /// A name for the Box; one is made up when left out.
    pub(super) name: Option<String>,
    /// What the Box's agent should do first.
    pub(super) prompt: Option<String>,
    /// The revision to branch from; the repo's default branch when left out.
    pub(super) base: Option<String>,
    /// A node `list_nodes` shows you; omit to start on the node `list_nodes` says a start picks.
    pub(super) node: Option<String>,
    /// A partial repo config, as YAML in the repo config format, laid over the repo's stored
    /// config for this Box only: a key you set wins, `env` merges by name, lists replace.
    /// `proxy.allow` adds extra `METHOD /path` entries, each with `{repo}`, to a default Proxy
    /// rule for this Box only.
    pub(super) config: Option<String>,
    /// Gates the Box must pass before it counts as done, in the order they pass: each is
    /// evidence files the agent leaves, checks moor runs in the Box (a command, or `{run, says}` with
    /// a plain sentence of what must be true), and an owner approval. A Box
    /// with an open gate shows as working, never as done. Written once, at start.
    pub(super) gates: Option<Vec<moor_core::gate::GateDoc>>,
    /// The outcomes the Box's run may end in, sent with `gates`; see `add_workflow`. Left out,
    /// the run ends in `done` once every gate passes.
    pub(super) outcomes: Option<Vec<moor_core::gate::OutcomeDoc>>,
    /// Wait for setup and the agent launch to finish before returning; false by default, so
    /// the call returns once the Box exists and `box_status` shows its progress.
    pub(super) wait: Option<bool>,
}

/// Arguments of `ask`.
#[derive(Deserialize, JsonSchema)]
pub(super) struct AskArgs {
    /// The repos the question is about, each as `owner/name`, first one first: the task's own.
    pub(super) repos: Vec<String>,
    /// The question, as you would put it to the person's agent.
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
