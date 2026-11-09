//! Keep when each of a Box's hands ended, so hands that failed or lost their VM stay as history
//! beside the new hands that replace them.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::int;

/// Add the nullable `box_hands.ended_at`.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("box_hands")
            .add_column(int("ended_at"))
            .take(),
    );
}
