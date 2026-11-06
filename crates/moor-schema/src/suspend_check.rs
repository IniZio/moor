//! Keep what a Box held that was not pushed when it was suspended, so a close can answer for a
//! Box too big to wake. A null `suspend_checked_at` means no answer stands.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::int;

/// Add the `suspend_ahead`, `suspend_dirty` and `suspend_checked_at` columns.
pub(super) fn write(s: &mut Script) {
    for column in ["suspend_ahead", "suspend_dirty", "suspend_checked_at"] {
        s.schema(&Table::alter().table("boxes").add_column(int(column)).take());
    }
}
