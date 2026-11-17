//! clap definitions for `moor node`.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use moor_core::request::AsReq;
use moor_core::Request;

use super::{Action, NodeAddSpec};

/// Subcommands of `node`.
#[derive(Debug, Subcommand)]
pub(super) enum NodeCmd {
    /// List the shared nodes and your own, with their load.
    Ls,
    /// Enroll a machine agent on this host as a shared node, with no sign-in (operator only).
    Add(AddArgs),
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
            Self::Add(a) => Action::NodeAdd(NodeAddSpec {
                name: a.name,
                agent_home: a.agent_home,
                runner: a.runner,
                labels: a.labels,
            }),
        }
    }
}
