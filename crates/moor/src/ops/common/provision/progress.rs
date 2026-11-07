//! What the driver reports while it acquires a Box, kept as the Box's events.

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::PostCloneStep;
use moor_core::seam::driver::{AcquireReq, Acquired, DriverError};
use moor_core::BoxId;

use crate::cx::Cx;
use crate::ops::common::setup::LINK_WAIT;
use crate::ops::common::step::timed_result;
use crate::store::Store;

/// Acquires the machine for Box `id`, recording each note the driver hands over and the time the
/// whole call took.
pub(super) fn acquire_timed(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    req: &AcquireReq,
) -> Result<Acquired, DriverError> {
    timed_result(cx, store, id, PostCloneStep::Acquire, || {
        cx.driver().acquire_noting(req, &mut progress(store, id))
    })
}

/// Pause before asking a host that has not relinked again.
const RELINK_POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// Takes over the acquire of Box `id` that a cut-off process left going, timed as an acquire. The
/// host may not have relinked yet, which is not the acquire failing, so it is asked again.
pub(super) fn adopt_timed(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    req: &AcquireReq,
) -> Result<Acquired, DriverError> {
    timed_result(cx, store, id, PostCloneStep::Acquire, || {
        let began = cx.clock().instant();
        loop {
            let got = cx.driver().adopt_noting(req, &mut progress(store, id));
            let waited = cx.clock().instant().saturating_duration_since(began);
            match got {
                Err(e) if e.to_string().contains("is not linked") && waited < LINK_WAIT => {
                    cx.clock().sleep(RELINK_POLL);
                }
                got => return got,
            }
        }
    })
}

/// Records each note the driver hands over while it acquires Box `id`.
fn progress(store: &Store, id: BoxId) -> impl FnMut(&str) + '_ {
    move |text| {
        let text = text.to_owned();
        drop(store.apply(Decision::new(id).event(ExtraEvent::BoxProgress { text })));
    }
}
