//! Arguments of the `events` verb.

use clap::Args;
use moor_core::controller::WatchFilter;
use moor_core::{BoxName, SessionId};

use super::box_name;

/// Print stored events as JSON lines; --follow keeps streaming (dev tool).
#[derive(Debug, Args)]
pub(super) struct EventsArgs {
    /// Only events after this id.
    #[arg(long, default_value_t = 0)]
    since: i64,
    /// Only this Box; repeatable.
    #[arg(long = "box", value_name = "NAME", value_parser = box_name)]
    boxes: Vec<BoxName>,
    /// Only this event kind; repeatable.
    #[arg(long = "kind", value_name = "KIND")]
    kinds: Vec<String>,
    /// Only events of this Session's child Sessions.
    #[arg(long, value_name = "SESSION")]
    parent: Option<i64>,
    /// Keep streaming new events.
    #[arg(long)]
    follow: bool,
}

impl EventsArgs {
    /// The since id, the filter and the follow flag.
    pub(super) fn into_parts(self) -> (i64, WatchFilter, bool) {
        let filter = WatchFilter {
            boxes: (!self.boxes.is_empty()).then_some(self.boxes),
            kinds: (!self.kinds.is_empty()).then_some(self.kinds),
            parent: self.parent.map(SessionId::new),
        };
        (self.since, filter, self.follow)
    }
}
