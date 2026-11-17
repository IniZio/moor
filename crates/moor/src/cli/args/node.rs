//! clap definitions for `moor node`.

use std::num::NonZeroU32;
use std::path::PathBuf;

use clap::{ArgGroup, Args, Subcommand};
use moor_core::request::{AsReq, CapEdit, NodeSettingsSetReq};
use moor_core::Request;

use super::{Action, NodeAddSpec};

/// Subcommands of `node`.
#[derive(Debug, Subcommand)]
pub(super) enum NodeCmd {
    /// List the shared nodes and your own, with their load.
    Ls,
    /// Enroll a machine agent on this host as a shared node, with no sign-in (operator only).
    Add(AddArgs),
    /// Set a node's cap or labels: your own node, or a shared one as the operator. The next
    /// start reads it.
    Set(SetArgs),
}

/// Arguments of `node set`.
#[derive(Debug, Args)]
#[command(group(ArgGroup::new("change").required(true).multiple(true)
    .args(["max_boxes", "no_cap", "labels", "no_labels"])))]
pub(super) struct SetArgs {
    /// The node's name.
    name: String,
    /// Hold at most this many live Boxes; more wait for room.
    #[arg(long, value_name = "N", conflicts_with = "no_cap")]
    max_boxes: Option<NonZeroU32>,
    /// Remove the cap, so free memory alone decides.
    #[arg(long)]
    no_cap: bool,
    /// A label a Box may require of the node; repeat for more. Replaces its labels.
    #[arg(long = "label", value_name = "LABEL", conflicts_with = "no_labels")]
    labels: Vec<String>,
    /// Remove every label.
    #[arg(long)]
    no_labels: bool,
    /// Refuse unless the settings are at revision N (0 for never written), so a change made since
    /// you read them is not overwritten.
    #[arg(long, value_name = "N")]
    expected_revision: Option<u64>,
}

/// Arguments of `node add`.
#[derive(Debug, Args)]
pub(super) struct AddArgs {
    /// The node's name.
    name: String,
    /// The `HOME` the agent runs under; its key and state are made there.
    #[arg(long, value_name = "DIR")]
    agent_home: PathBuf,
    /// Where the machine runner takes agent links (`moor-machine-runner --agents`), as
    /// `host:port` or a URL; plain http only on loopback.
    #[arg(long, value_name = "ADDR")]
    runner: String,
    /// A label a Box may require of the node; repeat for more.
    #[arg(long = "label", value_name = "LABEL")]
    labels: Vec<String>,
}

impl NodeCmd {
    /// What the subcommand asks for.
    pub(super) fn into_action(self) -> Action {
        match self {
            Self::Ls => Action::Request(Request::NodeList(AsReq { identity: None })),
            Self::Set(a) => Action::Request(Request::NodeSettingsSet(NodeSettingsSetReq {
                identity: None,
                name: a.name,
                max_boxes: match (a.max_boxes, a.no_cap) {
                    (Some(n), _) => CapEdit::Set(n),
                    (None, true) => CapEdit::Clear,
                    (None, false) => CapEdit::Keep,
                },
                labels: (a.no_labels || !a.labels.is_empty()).then_some(a.labels),
                expected_revision: a.expected_revision,
            })),
            Self::Add(a) => Action::NodeAdd(NodeAddSpec {
                name: a.name,
                agent_home: a.agent_home,
                runner: a.runner,
                labels: a.labels,
            }),
        }
    }
}
