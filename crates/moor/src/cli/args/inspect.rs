//! Arguments of the `inspect` verb.

use clap::Args;
use moor_core::request::InspectReq;
use moor_core::BoxName;

use super::box_name;

/// Arguments of `inspect`.
#[derive(Debug, Args)]
pub(super) struct InspectArgs {
    /// Box name.
    #[arg(value_parser = box_name)]
    name: BoxName,
    /// Pane lines to show; 40 by default, at most 200.
    #[arg(long, value_name = "N")]
    lines: Option<u32>,
    /// Agent messages to show; 5 by default, at most 20.
    #[arg(long, value_name = "N")]
    said: Option<u32>,
}

impl From<InspectArgs> for InspectReq {
    fn from(a: InspectArgs) -> Self {
        Self {
            target: a.name.into(),
            lines: a.lines,
            said: a.said,
        }
    }
}
