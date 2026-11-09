//! Keep where each Session's newest archive of the agent's project directory is, and which of
//! the Box's hands made it, so new hands can restore the conversation; and mark a Box whose
//! archive is due after a turn, so a save that could not run is retried.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{int, text};

/// Add the Session archive pointer and the Box's `archive_due`, all nullable.
pub(super) fn write(s: &mut Script) {
    for col in [text("archive_key"), int("archive_inc"), int("archive_at")] {
        s.schema(&Table::alter().table("sessions").add_column(col).take());
    }
    s.schema(
        &Table::alter()
            .table("box_state")
            .add_column(int("archive_due"))
            .take(),
    );
}
