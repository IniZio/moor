//! clap definitions for `moor node`.

use clap::Subcommand;
use moor_core::request::AsReq;
use moor_core::Request;

/// Subcommands of `node`.
#[derive(Debug, Subcommand)]
pub(super) enum NodeCmd {
    /// List the shared nodes and your own, with their load.
    Ls,
}

impl From<NodeCmd> for Request {
    fn from(cmd: NodeCmd) -> Self {
        match cmd {
            NodeCmd::Ls => Self::NodeList(AsReq { identity: None }),
        }
    }
}
