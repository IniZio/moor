//! The quick reads after a steer: the steered Box's agent is read every [`FOLLOW_EVERY`] until a
//! reading shows it took the steer up, for at most [`FOLLOW_FOR`], so its first line is on record
//! without waiting for the next settle pass.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

use moor_core::activity::Status;
use moor_core::domain::hands::HandsState;
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, Warning};

use super::{read, settle, turn_open, Reading, PROBE_DEADLINE};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Time between two reads of a steered Box.
pub(crate) const FOLLOW_EVERY: Duration = Duration::from_millis(250);

/// Longest a steered Box is read at [`FOLLOW_EVERY`]; the settle pass reads it after that.
pub(crate) const FOLLOW_FOR: Duration = Duration::from_secs(15);

/// The steered Boxes the controller has yet to follow, shared on its environment.
#[derive(Debug, Default)]
pub(crate) struct Steered {
    /// Boxes steered since the controller last looked, oldest first.
    boxes: Mutex<VecDeque<BoxId>>,
    /// Signalled when a Box is added.
    added: Condvar,
}

impl Steered {
    /// The next steered Box, waiting up to `wait` for one; none when none came.
    pub(crate) fn next(&self, wait: Duration) -> Option<BoxId> {
        let boxes = self.boxes.lock().unwrap_or_else(PoisonError::into_inner);
        self.added
            .wait_timeout_while(boxes, wait, |b| b.is_empty())
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .pop_front()
    }

    /// Adds Box `id`.
    fn add(&self, id: BoxId) {
        (self.boxes.lock())
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(id);
        self.added.notify_one();
    }
}

/// Has the controller follow Box `id`, just steered; outside a controller, nothing.
pub(crate) fn steered(cx: &Cx, id: BoxId) {
    if let Some(steered) = cx.env().shared::<std::sync::Arc<Steered>>() {
        steered.add(id);
    }
}

/// Reads and settles Box `id` every [`FOLLOW_EVERY`] until a reading shows its agent working on a
/// line or the steered turn over, a read fails, or [`FOLLOW_FOR`] has passed.
pub(crate) fn follow(cx: &Cx, store: &Store, id: BoxId) {
    let until = cx.clock().instant() + FOLLOW_FOR;
    loop {
        let Ok(Some(row)) = store.box_row(id) else {
            return;
        };
        if row.hands.as_ref().map(|h| h.state) != Some(HandsState::Ready) {
            return;
        }
        let settled = cx.for_box(row.user, || {
            let reading = read(cx, store, &row, cx.clock().now(), PROBE_DEADLINE)?;
            settle(cx, store, &row, &reading)?;
            Ok::<_, CommandError>(reading)
        });
        let reading = match settled {
            Ok(reading) => reading,
            Err(e) => {
                cx.warn(Warning {
                    code: "settle_failed".to_owned(),
                    message: format!("could not settle Box {}: {}", id.get(), e.error),
                });
                return;
            }
        };
        if reading.read_failed
            || taken_up(store, &row, &reading, cx.clock().now())
            || cx.clock().instant() >= until
        {
            return;
        }
        cx.clock().sleep(FOLLOW_EVERY);
    }
}

/// Whether `reading` shows the agent working on a line, or the last steered turn of `row` over.
fn taken_up(store: &Store, row: &BoxRow, reading: &Reading, now: Ts) -> bool {
    let status = reading.term.as_ref().and_then(|t| t.status.as_ref());
    matches!(status, Some(Status::Working { doing: Some(_) }))
        || !turn_open(store, row, reading, now).unwrap_or(false)
}
