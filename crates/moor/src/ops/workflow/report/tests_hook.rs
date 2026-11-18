//! A test seam between a report's reading of the run and its move.

use std::cell::RefCell;

use crate::cx::Cx;

type Hook = Box<dyn FnOnce(&Cx)>;

thread_local! {
    static AFTER_STANDING: RefCell<Option<Hook>> = const { RefCell::new(None) };
}

/// Runs `f` once, after the next reading of a run by a report on this thread.
pub(in crate::ops::workflow) fn once_after_standing(f: impl FnOnce(&Cx) + 'static) {
    AFTER_STANDING.with(|h| *h.borrow_mut() = Some(Box::new(f)));
}

pub(super) fn after_standing(cx: &Cx) {
    if let Some(f) = AFTER_STANDING.with(|h| h.borrow_mut().take()) {
        f(cx);
    }
}
