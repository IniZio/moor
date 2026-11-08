//! The arguments the gate's MCP tools take.

use schemars::JsonSchema;
use serde::Deserialize;

/// Arguments of `start_box`.
#[derive(Deserialize, JsonSchema)]
pub(super) struct StartArgs {
    /// The repo to clone, as `owner/name`.
    pub(super) repo: String,
    /// More repos to clone beside `repo`, each as `owner/name` and each one you can push to;
    /// your configured `box.also` when left out, none when empty.
    pub(super) also: Option<Vec<String>>,
    /// A name for the Box; one is made up when left out.
    pub(super) name: Option<String>,
    /// What the Box's agent should do first.
    pub(super) prompt: Option<String>,
    /// The revision to branch from; the repo's default branch when left out.
    pub(super) base: Option<String>,
    /// A node `list_nodes` shows you; omit to start on the node `list_nodes` says a start picks.
    pub(super) node: Option<String>,
    /// Set the Box up from scratch, past any setup kept from earlier Boxes; false by default.
    #[serde(default)]
    pub(super) cold: Option<bool>,
    /// Wait for setup and the agent launch to finish before returning; false by default, so
    /// the call returns once the Box exists and `box_status` shows its progress.
    pub(super) wait: Option<bool>,
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
