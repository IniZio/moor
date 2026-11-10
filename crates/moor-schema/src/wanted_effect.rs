//! Keep what moor wants a Box to end as, and the external step in flight for it, in columns of
//! the Box's own row, so no decision about the Box reads its event history as state.
//!
//! `box_state` gains the wanted action, the effect in flight, a note owed to the agent, when the
//! relaunch cap was recorded and whether the Box was parked mid-turn. `launches` gains why each
//! launch began, on which hands, and the agent facts that were events; `proxy` gains its last
//! tunnel restart. The columns start empty: the binary that first reads each one also fills it,
//! in a step of its own, so nothing is filled from a state a later deploy has moved past.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{int, text};

/// The `box_state` columns this step adds, in order.
const STATE: [(&str, bool); 11] = [
    ("wanted", true),
    ("wanted_force", false),
    ("wanted_reason", true),
    ("wanted_at", false),
    ("effect", true),
    ("effect_attempt", false),
    ("effect_run", false),
    ("effect_at", false),
    ("owed_note", true),
    ("relaunch_capped_at", false),
    ("parked_at_work", false),
];

/// The `launches` columns this step adds, in order.
const LAUNCH: [(&str, bool); 5] = [
    ("cause", true),
    ("hands_inc", false),
    ("stalled_at", false),
    ("unreachable_since", false),
    ("exit_save_at", false),
];

/// Add the columns, every one nullable and empty.
pub(super) fn write(s: &mut Script) {
    for (table, cols) in [("box_state", &STATE[..]), ("launches", &LAUNCH[..])] {
        for (name, is_text) in cols {
            let col = if *is_text { text(name) } else { int(name) };
            s.schema(&Table::alter().table(table).add_column(col).take());
        }
    }
    s.schema(
        &Table::alter()
            .table("proxy")
            .add_column(int("restarted_at"))
            .take(),
    );
}
