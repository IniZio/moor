//! Releasing a Box's machine on close.

use moor_core::decide::close::{release_outcome_with_reason, ReleaseResult};
use moor_core::decide::reconcile::{reconcile, BoxFact};
use moor_core::domain::decision::{Decision, Effect, EffectChange};
use moor_core::seam::driver::Released;
use moor_core::ErrorKind;

use super::held;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Whether the provider's inventory no longer lists the Box's machine, as reconcile reads it.
///
/// A listing that fails or is unsupported cannot tell, so the close goes on as usual.
pub(super) fn machine_vanished(cx: &Cx, row: &BoxRow) -> bool {
    let Some(b) = row.hands.as_ref() else {
        return false;
    };
    let fact = BoxFact {
        box_id: row.id,
        inc: b.inc,
        state: b.state,
        handle: b.handle.clone(),
    };
    cx.driver()
        .list(&b.provider)
        .is_ok_and(|listing| !reconcile(&[fact], &listing).is_empty())
}

/// Asks the Driver to release the Box and records the verdict. The release is the Box's effect in
/// flight from just before the Driver is asked, so a restart that cuts it releases again, and a
/// release that fails ends it with the Box left open.
///
/// Returns whether the Box is now deleted.
pub(super) fn release(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    reason: Option<String>,
) -> Result<bool, CommandError> {
    let Some(b) = row.hands.as_ref() else {
        return Ok(false);
    };
    let Some(handle) = held(b) else {
        return Ok(false);
    };
    store.apply(Decision::new(row.id).effect(EffectChange::Next(Effect::Release)))?;
    let (result, cause) = match cx.driver().release(handle) {
        Ok(Released::Deleted | Released::AlreadyGone) => (ReleaseResult::Deleted, None),
        Err(e) => (ReleaseResult::Failed, Some(e)),
    };
    let verdict = release_outcome_with_reason(row.id, (b.inc, b.state), result, reason);
    if let Some(d) = verdict.decision {
        store.apply(d)?;
    }
    if let Some(w) = verdict.warning {
        cx.warn(w);
    }
    if !verdict.closes {
        store.apply(Decision::new(row.id).effect(EffectChange::End))?;
        let why = cause.map_or_else(
            || "the Box could not be released".to_owned(),
            |e| e.to_string(),
        );
        let by_hand = cx.driver().manual_release(handle).map_or_else(
            || "release the Box by hand".to_owned(),
            |how| format!("release the Box by hand: {how}"),
        );
        return Err(
            CommandError::new(ErrorKind::Failed, format!("{why}; the Box stays open"))
                .with_hint(format!("try `moor close` again, or {by_hand}")),
        );
    }
    Ok(result == ReleaseResult::Deleted)
}
