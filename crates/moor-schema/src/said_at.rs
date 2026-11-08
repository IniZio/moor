//! Keep when the idle status whose last reply was recorded was set, so a reply is recorded once.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::int;

/// Add the launches' `said_at` column.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("launches")
            .add_column(int("said_at"))
            .take(),
    );
}
