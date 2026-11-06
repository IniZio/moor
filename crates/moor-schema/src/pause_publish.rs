//! Keep whether a Box's last suspend let its provider save its caches. Null means it did not.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::int;

/// Add the `suspend_publish` column.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("boxes")
            .add_column(int("suspend_publish"))
            .take(),
    );
}
