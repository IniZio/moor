//! The hosts a Docker Box reached. Every [`READ_EVERY`] the keeper reads the rows the machine
//! agent keeps in the Box and writes the new ones into an `egress_seen` event.

use std::time::Duration;

use moor_core::box_key::is_machine_handle;
use moor_core::domain::egress::{EgressFlushed, EgressRow, SEEN_FILE};
use moor_core::domain::event::Event;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::Place;
use moor_core::{BoxHandle, BoxPath};

use super::{lock, Kept};

/// Pause between reads of a Box's rows.
pub(super) const READ_EVERY: Duration = Duration::from_secs(30);

/// What the keeper holds between reads.
#[derive(Default)]
pub(super) struct Egress {
    /// What the Box's events already carry.
    flushed: EgressFlushed,
    /// When the next read is due; none before the first.
    next: Option<Ts>,
}

impl Kept {
    /// Reads the rows of the Box on `handle` when a read is due, and writes the new ones into an
    /// `egress_seen` event. Only a machine's Box keeps rows.
    pub(super) fn read_egress(&self, handle: &BoxHandle) {
        if !is_machine_handle(handle.as_str()) {
            return;
        }
        let now = self.deps.clock.now();
        {
            let mut egress = lock(&self.egress);
            if egress.next.is_some_and(|next| now < next) {
                return;
            }
            egress.next = Some(now.saturating_add(READ_EVERY));
        }
        let Ok(path) = BoxPath::restore(SEEN_FILE) else {
            return;
        };
        let text = match self.deps.driver.peek_file(handle, Place::Agent, &path) {
            Ok(Some(text)) => text,
            Ok(None) => return,
            Err(e) => return self.say(format_args!("cannot read {SEEN_FILE}: {e}")),
        };
        let rows: Vec<EgressRow> = match serde_json::from_str(&text) {
            Ok(rows) => rows,
            Err(e) => return self.say(format_args!("cannot parse {SEEN_FILE}: {e}")),
        };
        let at = u64::try_from(now.as_millis()).unwrap_or(0);
        let hosts = lock(&self.egress).flushed.take(&rows, at);
        if !hosts.is_empty() {
            self.record(&Event::EgressSeen { hosts });
        }
    }
}
