//! Keep when a Box's places last died, in its own row, so the controller can tell a Box that
//! keeps dying from one that died once.
//!
//! `box_state.recent_deaths` holds a JSON list of unix milliseconds, oldest first, and starts
//! empty: a Box from before the step has no recorded death.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::text;

/// Add `box_state.recent_deaths`.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("box_state")
            .add_column(text("recent_deaths"))
            .take(),
    );
}
