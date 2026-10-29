//! What the driver reports while it acquires a Box, kept as the Box's events.

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::BoxId;

use crate::store::Store;

/// Records each note the driver hands over while it acquires Box `id`.
pub(super) fn progress(store: &Store, id: BoxId) -> impl FnMut(&str) + '_ {
    move |text| {
        let text = text.to_owned();
        drop(store.apply(Decision::new(id).event(ExtraEvent::BoxProgress { text })));
    }
}
