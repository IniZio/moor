//! Records when each step after the clone starts and ends, and names a step that ran out of time.

use std::time::Duration;

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::PostCloneStep;
use moor_core::seam::driver::DriverError;
use moor_core::{BoxId, Warning};

use crate::cx::Cx;
use crate::store::Store;

/// Runs `f`, recording a start event before and an end event with the time it took as soon as it
/// returns. `f` says whether the step succeeded.
///
/// A failed write is warned, not returned: these events only report progress.
pub(crate) fn timed<T>(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    step: PostCloneStep,
    f: impl FnOnce() -> (T, bool),
) -> T {
    record(cx, store, id, ExtraEvent::StepStarted { step });
    let started = cx.clock().instant();
    let (out, ok) = f();
    let elapsed = cx.clock().instant().saturating_duration_since(started);
    let ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    record(cx, store, id, ExtraEvent::StepFinished { step, ms, ok });
    out
}

/// [`timed`] for a step that succeeds when `f` returns `Ok`.
pub(crate) fn timed_result<T, E>(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    step: PostCloneStep,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    timed(cx, store, id, step, || {
        let out = f();
        let ok = out.is_ok();
        (out, ok)
    })
}

/// Writes `event` for Box `id`.
fn record(cx: &Cx, store: &Store, id: BoxId, event: ExtraEvent) {
    if let Err(e) = store.apply(Decision::new(id).event(event)) {
        cx.warn(Warning {
            code: "step_event_failed".to_owned(),
            message: format!("could not record setup progress: {e}"),
        });
    }
}

/// Why `what` could not run to an exit: names the step and the limit when the deadline hit.
pub(crate) fn exec_failure(what: &str, error: &DriverError) -> String {
    match error {
        DriverError::Timeout(limit) => format!("{what} ran past {}", limit_text(*limit)),
        other => other.to_string(),
    }
}

/// `limit` in minutes when it is a whole number of them, else in seconds.
fn limit_text(limit: Duration) -> String {
    let secs = limit.as_secs();
    if secs >= 60 && secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deadline_names_the_step_and_the_limit() {
        let hit = DriverError::Timeout(Duration::from_mins(15));
        assert_eq!(
            exec_failure("dotfiles install", &hit),
            "dotfiles install ran past 15m"
        );
        let hit = DriverError::Timeout(Duration::from_secs(90));
        assert_eq!(
            exec_failure("profile fetch", &hit),
            "profile fetch ran past 90s"
        );
    }

    #[test]
    fn another_driver_error_keeps_its_own_text() {
        let e = DriverError::Unavailable("ssh down".into());
        assert_eq!(exec_failure("agent install", &e), e.to_string());
    }
}
