//! clap definitions for `moor steer`.

use clap::Args;
use moor_core::{BoxName, SteerReq};

use super::box_name;

/// Arguments of `steer`.
#[derive(Debug, Args)]
pub(super) struct SteerArgs {
    /// Box name.
    #[arg(value_parser = box_name)]
    name: BoxName,
    /// Text to send.
    #[arg(conflicts_with_all = ["menu", "interrupt"])]
    text: Option<String>,
    /// Menu choice to send.
    #[arg(
        long,
        required_unless_present_any = ["text", "interrupt"],
        conflicts_with = "interrupt",
        value_name = "N"
    )]
    menu: Option<u32>,
    /// Send the agent's interrupt key, which stops its current work.
    #[arg(long)]
    interrupt: bool,
}

impl From<SteerArgs> for SteerReq {
    fn from(a: SteerArgs) -> Self {
        Self {
            target: a.name.into(),
            text: a.text,
            menu: a.menu,
            interrupt: a.interrupt,
        }
    }
}
