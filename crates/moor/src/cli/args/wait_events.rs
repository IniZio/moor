//! Arguments of `wait-events`.

use clap::Args;
use moor_core::{BoxName, WaitEventsReq, WaitMode};

use super::task_name;

/// Arguments of `wait-events`.
#[derive(Debug, Args)]
pub(super) struct WaitEventsArgs {
    /// Box names to watch.
    #[arg(required = true, value_parser = task_name)]
    boxes: Vec<BoxName>,
    /// Event kind to match; repeatable.
    #[arg(long = "kind", value_name = "KIND")]
    kinds: Vec<String>,
    /// Wait for every Box instead of any.
    #[arg(long)]
    all: bool,
    /// Only events after this id.
    #[arg(long, value_name = "N")]
    since: Option<i64>,
    /// Timeout in seconds.
    #[arg(long, value_name = "S")]
    timeout: Option<u64>,
}

impl From<WaitEventsArgs> for WaitEventsReq {
    fn from(a: WaitEventsArgs) -> Self {
        Self {
            boxes: a.boxes,
            kinds: (!a.kinds.is_empty()).then_some(a.kinds),
            mode: if a.all { WaitMode::All } else { WaitMode::Any },
            since: a.since,
            timeout_secs: a.timeout,
        }
    }
}
