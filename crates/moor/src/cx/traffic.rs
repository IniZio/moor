//! When bytes last moved through each Box's port forwards, which holds auto-sleep off.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use moor_core::name::BoxId;
use moor_core::seam::clock::Ts;

/// How long after a forward's last byte its Box counts as in use.
pub(crate) const HOLD: Duration = Duration::from_mins(10);

/// How many Boxes the record holds before a touch drops those past [`HOLD`].
const PRUNE_AT: usize = 64;

/// The last time a byte moved through each Box's forwards; one per controller.
#[derive(Default)]
pub(crate) struct Traffic {
    /// When, by Box.
    last: Mutex<HashMap<BoxId, Ts>>,
}

impl Traffic {
    /// Notes that bytes moved through a forward of Box `id` at `now`.
    pub(crate) fn touch(&self, id: BoxId, now: Ts) {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if last.len() >= PRUNE_AT {
            last.retain(|_, at| now < at.saturating_add(HOLD));
        }
        last.insert(id, now);
    }

    /// Whether bytes moved through a forward of Box `id` within [`HOLD`] of `now`.
    pub(crate) fn recent(&self, id: BoxId, now: Ts) -> bool {
        self.last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .is_some_and(|&at| now < at.saturating_add(HOLD))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_counts_as_in_use_for_ten_minutes_after_its_last_byte() {
        let traffic = Traffic::default();
        let id = BoxId::new(7);
        let at = Ts::from_millis(1_000_000);
        assert!(!traffic.recent(id, at));
        traffic.touch(id, at);
        assert!(traffic.recent(id, at.saturating_add(Duration::from_secs(599))));
        assert!(!traffic.recent(id, at.saturating_add(Duration::from_mins(10))));
        assert!(!traffic.recent(BoxId::new(8), at));
    }

    #[test]
    fn boxes_past_the_hold_are_dropped_once_the_record_grows() {
        let traffic = Traffic::default();
        let old = Ts::from_millis(0);
        for n in 0..64_i64 {
            traffic.touch(BoxId::new(n), old);
        }
        traffic.touch(
            BoxId::new(1000),
            old.saturating_add(Duration::from_mins(11)),
        );
        let held = traffic.last.lock().unwrap().len();
        assert_eq!(held, 1);
    }
}
