//! Let a person take an item out of their inbox. The row stays, marked, so the event it was
//! written for cannot add it again.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::int;

/// Add the nullable `user_inbox.cleared_at`: when the person cleared the item, in unix
/// milliseconds, or null while it is listed.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("user_inbox")
            .add_column(int("cleared_at"))
            .take(),
    );
}
