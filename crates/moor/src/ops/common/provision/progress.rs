//! What the driver reports while it acquires a Box, kept as the Box's events.

use std::time::Instant;

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::PostCloneStep;
use moor_core::seam::driver::{AcquireReq, Acquired, DriverError};
use moor_core::BoxId;

use crate::cx::Cx;
use crate::ops::common::setup::{out_of_reach, LINK_WAIT};
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
        match cx.driver().acquire_noting(req, &mut progress(store, id)) {
            Err(e) if goes_on_after(cx, req, &e) => {
                let gap = out_of_reach(&e).map(|why| reach_lost(cx, store, id, why));
                take_over(cx, store, id, req, gap)
            }
            got => got,
        }
    })
}

/// Whether `e` cut off an acquire that goes on without its caller: the machine went out of reach
/// while it made the Box.
pub(in crate::ops::common::provision) fn goes_on_after(
    cx: &Cx,
    req: &AcquireReq,
    e: &DriverError,
) -> bool {
    out_of_reach(e).is_some()
        && cx
            .driver()
            .acquire_outlives_caller(&cx.driver().planned_handle(req))
}

/// Takes over the acquire of Box `id` that a cut-off process left going, timed as an acquire.
pub(super) fn adopt_timed(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    req: &AcquireReq,
) -> Result<Acquired, DriverError> {
    timed_result(cx, store, id, PostCloneStep::Acquire, || {
        take_over(cx, store, id, req, None)
    })
}

/// Pause before asking a host out of reach again.
const RELINK_POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// Takes over the acquire going on for Box `id`, whose host has been out of reach since `gap`
/// when it is. A host out of reach is not the acquire failing, so it is asked again until it has
/// stayed out of reach for [`LINK_WAIT`], or until a drain leaves it to the next process. A call
/// that hears from the host ends the gap; each new gap is an `acquire_reattached` event.
pub(in crate::ops::common::provision) fn take_over(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    req: &AcquireReq,
    mut gap: Option<Instant>,
) -> Result<Acquired, DriverError> {
    loop {
        let mut heard = false;
        let got = {
            let mut note = progress(store, id);
            cx.driver().adopt_noting(req, &mut |text| {
                heard = true;
                note(text);
            })
        };
        let lost = match &got {
            Err(e) => out_of_reach(e).map(str::to_owned),
            Ok(_) => None,
        };
        let Some(why) = lost else {
            return got;
        };
        if heard {
            gap = None;
        }
        let since = *gap.get_or_insert_with(|| reach_lost(cx, store, id, &why));
        let waited = cx.clock().instant().saturating_duration_since(since);
        let stopped = cx.env().draining().is_cancelled() || cx.env().cancel().is_cancelled();
        if stopped || waited >= LINK_WAIT {
            return got;
        }
        cx.clock().sleep(RELINK_POLL);
    }
}

/// Records that Box `id`'s host went out of reach mid-acquire for `why`; when it did.
fn reach_lost(cx: &Cx, store: &Store, id: BoxId, why: &str) -> Instant {
    let why = why.to_owned();
    drop(store.apply(Decision::new(id).event(ExtraEvent::AcquireReattached { why })));
    cx.clock().instant()
}

/// Records each note the driver hands over while it acquires Box `id`.
fn progress(store: &Store, id: BoxId) -> impl FnMut(&str) + '_ {
    move |text| {
        let text = text.to_owned();
        drop(store.apply(Decision::new(id).event(ExtraEvent::BoxProgress { text })));
    }
}
