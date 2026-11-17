//! A run that waits at an approve step, as the tick sees it.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{RunOutcome, StepId, Workflow, WorkflowRun};
use moor_core::BoxId;

use crate::store::{Store, StoreError};

/// Ends the run that waits at `at` as incomplete when its Box is gone or closed: nobody is left
/// to send the answer to. A Box that sleeps is what a wait wants, so any other Box leaves it be.
pub(super) fn lose_waiting(
    store: &Store,
    w: &Workflow,
    run: &WorkflowRun,
    (box_id, at, try_n): (BoxId, &StepId, u8),
    now: Ts,
) -> Result<(), StoreError> {
    let row = store.box_row(box_id)?;
    if row.is_some_and(|r| !matches!(r.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected)) {
        return Ok(());
    }
    let key = (w.id, &run.repo, run.due_at);
    let outcome = RunOutcome::Incomplete { at: at.clone() };
    store
        .end_waiting_run(key, (at, try_n), (box_id, outcome), (&[], now))
        .map(|_| ())
}

/// A test seam between the tick's history read and the rest of its settling.
#[cfg(test)]
pub(super) mod tests_hook {
    use std::cell::RefCell;

    use crate::cx::Cx;

    type Hook = Box<dyn Fn(&Cx)>;

    thread_local! {
        static AFTER_HISTORY: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    /// Runs `f` after each history read of this thread, until the returned guard drops.
    pub(in crate::ops::workflow) fn on_history(f: impl Fn(&Cx) + 'static) -> Guard {
        AFTER_HISTORY.with(|h| *h.borrow_mut() = Some(Box::new(f)));
        Guard
    }

    pub(in crate::ops::workflow) fn after_history(cx: &Cx) {
        let hook = AFTER_HISTORY.with(|h| h.borrow_mut().take());
        if let Some(f) = hook {
            f(cx);
            AFTER_HISTORY.with(|h| *h.borrow_mut() = Some(f));
        }
    }

    pub(in crate::ops::workflow) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            AFTER_HISTORY.with(|h| *h.borrow_mut() = None);
        }
    }
}
